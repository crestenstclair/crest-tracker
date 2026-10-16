//! A sequential listening tour of the installed registries. All scheduling,
//! navigation, preparation, and reporting live on the control thread.
use crate::control::{
    AppEvent, AppLoop, AppState, Direction, EventSource, InteractionMode, PatchControlId,
    SavedSession, SemanticAction, SessionCandidateRequest, SessionCandidateResult,
    SessionCandidateSource, SessionCandidateToken, SessionCandidateWorker, TopLevelContext,
};
use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crate::kernel::{MidiChannel, PatchId};
use crate::mixer::{global_parameters::GlobalParameters, patch_output::PatchOutput};
use crate::real_time::{ControlAudioBoundary, GraphRevision};
use crate::shell::{SessionDocumentMarker, SessionDocumentProjection};
use crate::synth::effect_slot_id::EffectSlotIndex;
use crate::synth::{
    CapabilityId, DescriptorDefaultConfigFactory, EffectCapabilityId, EffectCapabilityRegistry,
    EffectSlotId, InstrumentConfig, ParameterAdjustment, ParameterAssignment, ParameterDefault,
    ParameterKind, ParameterSpec, ParameterUpdate, ParameterValue, Patch, PatchInteraction,
    PostEffectConfig,
};
use crate::testing::patch_control_navigation::next_patch_control_action;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

const BARS: usize = 8;
const NOTES_PER_BAR: usize = 8;
// Eighth notes at 120 BPM.
pub(crate) const NOTE_LENGTH: Duration = Duration::from_millis(250);
// The longest bundled instrument resampler needs about 160 ms at 48 kHz.
// Leave time for its output before releasing this one voice.
pub(crate) const NOTE_GATE: Duration = Duration::from_millis(200);
// Leave most of each 16 ms window tick available to input, projection, and MIDI.
const CONTROL_WORK_PER_TICK: Duration = Duration::from_millis(4);
const PREPARATION_TIMEOUT: Duration = Duration::from_secs(60);
// An original ascending/descending major-seventh arpeggio across three octaves.
// The startup test MIDI source also uses this phrase and its note timing.
// The same phrase, channel, velocity, and host envelope serve every audition.
pub(crate) const ARPEGGIO: [u8; 24] = [
    48, 52, 55, 59, 60, 64, 67, 71, 72, 76, 79, 83, 84, 83, 79, 76, 72, 71, 67, 64, 60, 59, 55, 52,
];

#[derive(Debug, thiserror::Error)]
#[error("full instrument/effect demo: {0}")]
pub struct FullDemoError(String);

pub(super) fn failure(error: impl std::fmt::Display) -> FullDemoError {
    FullDemoError(error.to_string())
}

/// Concrete reference and known functionality to omit are chosen by the
/// composition root. The scene still discovers content from the registries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FullDemoSelection {
    pub effect_reference: CapabilityId,
    pub skipped_instruments: Vec<CapabilityId>,
    pub skipped_effects: Vec<EffectCapabilityId>,
}

pub(crate) enum ListeningDemoSelection {
    Catalog(FullDemoSelection),
    DrumRack,
}

#[derive(Clone)]
pub(super) struct ParameterEdit {
    pub spec: ParameterSpec,
    pub value: ParameterValue,
}

#[derive(Clone)]
pub(super) struct DemoBar {
    pub label: String,
    pub session: SavedSession,
    pub control: PatchControlId,
    pub edit: Option<ParameterEdit>,
    pub prerequisites: Vec<ParameterEdit>,
    pub requires_preparation: bool,
    pub notes: Option<[Vec<(u8, u8)>; NOTES_PER_BAR]>,
}

struct ControlEdit {
    control: PatchControlId,
    edit: ParameterEdit,
    remember_original: bool,
}

pub(super) struct Audition {
    pub label: String,
    pub bars: Vec<DemoBar>,
}

pub(crate) struct FullDemoPlan {
    pub(super) auditions: Vec<Audition>,
    pub(super) instruments: usize,
    pub(super) effects: usize,
    pub(super) asset_notes: Vec<String>,
    pub(super) skipped: Vec<String>,
    pub(super) title: &'static str,
    pub(super) max_active_notes: usize,
    pub(super) introduction: Option<&'static str>,
}

impl FullDemoPlan {
    pub fn build(
        factory: &DescriptorDefaultConfigFactory,
        effects: &EffectCapabilityRegistry,
        selection: &FullDemoSelection,
    ) -> Result<Self, FullDemoError> {
        let reference = factory
            .create(&selection.effect_reference)
            .map_err(failure)?;
        let reference_label = factory
            .registry()
            .descriptor(&selection.effect_reference)
            .ok_or_else(|| failure("missing reference instrument descriptor"))?
            .label();
        let mut plan = Self {
            auditions: Vec::new(),
            instruments: 0,
            effects: 0,
            asset_notes: Vec::new(),
            skipped: Vec::new(),
            title: "Full instrument / effect demo",
            max_active_notes: 1,
            introduction: None,
        };
        for descriptor in factory.registry().descriptors() {
            if selection.skipped_instruments.contains(descriptor.id()) {
                plan.skipped.push(descriptor.label().to_owned());
                continue;
            }
            if !descriptor.availability().is_enabled() {
                return Err(failure(format!(
                    "instrument {} unavailable: {:?}",
                    descriptor.label(),
                    descriptor.availability()
                )));
            }
            let default = factory.create(descriptor.id()).map_err(failure)?;
            let label = format!("Instrument / {} / dry", descriptor.label());
            let baseline = Self::make_bar(
                factory,
                effects,
                "Defaults",
                &default,
                None,
                PatchControlId::Engine,
                None,
            )?;
            let mut bars = vec![baseline.clone()];
            plan.instruments += 1;
            let specs = descriptor.parameters().cloned().collect::<Vec<_>>();
            for spec in descriptor.parameters() {
                if spec.kind() == ParameterKind::Asset
                    || spec.patch_interaction() == PatchInteraction::ReadOnly
                {
                    plan.asset_notes.push(format!(
                        "{} / {}: bundled asset or read-only metadata",
                        descriptor.label(),
                        spec.label()
                    ));
                }
            }
            for edit in representative_edits(&specs)? {
                let spec = &edit.spec;
                let control = PatchControlId::Capability(spec.id().clone());
                let mut values = default.values().to_vec();
                let prerequisites =
                    enable_prerequisites(spec, &specs, &mut values, &mut Vec::new())?;
                let base = descriptor
                    .create_config(&values, default.asset_references())
                    .map_err(failure)?;
                if spec.update() == ParameterUpdate::Structural {
                    if let ParameterValue::Choice(choice) = &edit.value {
                        let config = factory
                            .replace_structural_choice(&base, spec.id(), choice)
                            .map_err(failure)?;
                        let mut bar = Self::make_bar(
                            factory,
                            effects,
                            &format!("{}: {}", spec.label(), display_value(spec, &edit.value)),
                            &config,
                            None,
                            control.clone(),
                            None,
                        )?;
                        bar.requires_preparation = true;
                        bars.push(bar);
                    } else {
                        return Err(failure(format!(
                            "{} has an unsupported structural edit",
                            spec.label()
                        )));
                    }
                } else {
                    let mut bar = Self::make_bar(
                        factory,
                        effects,
                        spec.label(),
                        &base,
                        None,
                        control.clone(),
                        Some(edit.clone()),
                    )?;
                    bar.requires_preparation = prerequisites
                        .iter()
                        .any(|edit| edit.spec.update() == ParameterUpdate::Structural);
                    bar.prerequisites = prerequisites;
                    bars.push(bar);
                }
            }
            bars.resize(BARS, baseline);
            plan.auditions.push(Audition { label, bars });
        }
        for descriptor in effects.descriptors() {
            if selection.skipped_effects.contains(descriptor.id()) {
                plan.skipped.push(descriptor.label().to_owned());
                continue;
            }
            if !descriptor.availability().is_enabled() {
                return Err(failure(format!(
                    "effect {} unavailable: {:?}",
                    descriptor.label(),
                    descriptor.availability()
                )));
            }
            let default = descriptor
                .default_config(EffectSlotId::new(1).map_err(failure)?)
                .map_err(failure)?;
            let label = format!("Effect / {} / {reference_label}", descriptor.label());
            let slot = EffectSlotIndex::new(0).map_err(failure)?;
            let baseline = Self::make_bar(
                factory,
                effects,
                "Defaults",
                &reference,
                Some(&default),
                PatchControlId::EffectSlot(slot),
                None,
            )?;
            let mut bars = vec![baseline.clone()];
            plan.effects += 1;
            let specs = descriptor.parameters().cloned().collect::<Vec<_>>();
            for spec in descriptor.parameters() {
                if spec.kind() == ParameterKind::Asset
                    || spec.patch_interaction() == PatchInteraction::ReadOnly
                {
                    plan.asset_notes.push(format!(
                        "{} / {}: bundled asset or read-only metadata",
                        descriptor.label(),
                        spec.label()
                    ));
                }
            }
            for edit in representative_edits(&specs)? {
                let spec = &edit.spec;
                if spec.update() != ParameterUpdate::Scalar {
                    return Err(failure(format!(
                        "{} / {} has an unsupported structural edit",
                        descriptor.label(),
                        spec.label()
                    )));
                }
                let mut values = default.values().to_vec();
                let prerequisites =
                    enable_prerequisites(spec, &specs, &mut values, &mut Vec::new())?;
                let base = descriptor
                    .create_config(default.slot_id(), &values, default.asset_references())
                    .map_err(failure)?;
                let mut bar = Self::make_bar(
                    factory,
                    effects,
                    spec.label(),
                    &reference,
                    Some(&base),
                    PatchControlId::Effect(base.slot_id(), spec.id().clone()),
                    Some(edit.clone()),
                )?;
                bar.requires_preparation = prerequisites
                    .iter()
                    .any(|edit| edit.spec.update() == ParameterUpdate::Structural);
                bar.prerequisites = prerequisites;
                bars.push(bar);
            }
            bars.resize(BARS, baseline);
            plan.auditions.push(Audition { label, bars });
        }
        if plan.auditions.is_empty() {
            return Err(failure("no instruments or effects selected for the demo"));
        }
        Ok(plan)
    }

    #[allow(clippy::too_many_arguments)]
    fn make_bar(
        factory: &DescriptorDefaultConfigFactory,
        effects: &EffectCapabilityRegistry,
        label: &str,
        instrument: &InstrumentConfig,
        effect: Option<&PostEffectConfig>,
        control: PatchControlId,
        edit: Option<ParameterEdit>,
    ) -> Result<DemoBar, FullDemoError> {
        let mut patch = Patch::new(
            PatchId::new(1).map_err(failure)?,
            label.to_owned(),
            instrument.clone(),
            MidiChannel::new(0).map_err(failure)?,
            PatchOutput::default(),
        );
        if let Some(effect) = effect {
            patch =
                patch.with_effect_slot(EffectSlotIndex::new(0).map_err(failure)?, effect.clone());
        }
        // No returns, sends, other Patches, or release tails. The gain is a
        // fixed listening level, not a change to any instrument's defaults.
        let mut state = AppState::new_with_effects(
            factory.registry().clone(),
            effects.clone(),
            GlobalParameters::new(-18.0).map_err(failure)?,
        );
        state
            .apply(AppEvent::InstallPatches(vec![patch]))
            .map_err(failure)?;
        state
            .apply(AppEvent::SelectContext(TopLevelContext::Patch))
            .map_err(failure)?;
        while let Some(action) =
            next_patch_control_action(&state, &PatchControlId::VoiceLimit).map_err(failure)?
        {
            state.apply_semantic_action(action).map_err(failure)?;
        }
        state
            .apply(AppEvent::SetInteractionMode(InteractionMode::Adjust))
            .map_err(failure)?;
        while state.patches()[0].voice_limit().value() > 1 {
            state
                .apply(AppEvent::Adjust(Direction::Down))
                .map_err(failure)?;
        }
        Ok(DemoBar {
            label: label.to_owned(),
            session: SavedSession::capture(&state),
            control,
            edit,
            prerequisites: Vec::new(),
            requires_preparation: false,
            notes: None,
        })
    }

    pub fn initial_session(&self) -> SavedSession {
        self.auditions[0].bars[0].session.clone()
    }

    fn announce(&self) {
        if let Some(introduction) = self.introduction {
            eprintln!("{}: {introduction}", self.title);
            for note in &self.asset_notes {
                eprintln!("Asset scope: {note}");
            }
            return;
        }
        eprintln!("New instrument/effect demo: {} instruments dry, then {} effects on the unchanged reference instrument. {} eight-bar auditions; {:.1} minutes of music plus preparation. 120 BPM, 4/4, one voice. Close the window or Ctrl-C to stop.", self.instruments, self.effects, self.auditions.len(), self.auditions.len() as f64 * 16.0 / 60.0);
        eprintln!("Skipped known functionality: {}.", self.skipped.join(", "));
        eprintln!("Eight bars total per entry: one default bar, then seven representative parameter/preset variations. Other settings reset before each bar. This is a quick tour, not exhaustive parameter or preset coverage.");
        for note in &self.asset_notes {
            eprintln!("Asset scope: {note}");
        }
        eprintln!("Imported file libraries are not enumerated. NAM uses its bundled upstream test model; convolution uses the bundled transparent IR.");
    }
}

fn enable_prerequisites(
    spec: &ParameterSpec,
    specs: &[ParameterSpec],
    values: &mut [ParameterAssignment],
    visiting: &mut Vec<crate::synth::ParameterId>,
) -> Result<Vec<ParameterEdit>, FullDemoError> {
    if visiting.contains(spec.id()) {
        return Err(failure("cyclic parameter prerequisites"));
    }
    visiting.push(spec.id().clone());
    let mut edits = Vec::new();
    for predicate in [spec.enabled_when(), spec.visible_when()]
        .into_iter()
        .flatten()
    {
        let dependency = specs
            .iter()
            .find(|candidate| candidate.id() == predicate.parameter_id())
            .ok_or_else(|| failure("unknown parameter prerequisite"))?;
        edits.extend(enable_prerequisites(dependency, specs, values, visiting)?);
        let assignment = values
            .iter_mut()
            .find(|value| value.parameter_id() == predicate.parameter_id())
            .ok_or_else(|| failure("missing prerequisite value"))?;
        *assignment =
            ParameterAssignment::new(predicate.parameter_id().clone(), predicate.equals().clone());
        edits.push(ParameterEdit {
            spec: dependency.clone(),
            value: predicate.equals().clone(),
        });
    }
    visiting.pop();
    Ok(edits)
}

/// Spread seven edits over the descriptor, revisiting controls with other
/// values when there are fewer than seven. Do not expand preset libraries or
/// parameter ranges into additional auditions.
fn representative_edits(specs: &[ParameterSpec]) -> Result<Vec<ParameterEdit>, FullDemoError> {
    let mut candidates = Vec::new();
    for spec in specs {
        if spec.kind() == ParameterKind::Asset
            || spec.patch_interaction() == PatchInteraction::ReadOnly
        {
            continue;
        }
        let mut values = representative_values(spec)?;
        values.dedup();
        if let ParameterDefault::Value(default) = spec.default_value() {
            values.retain(|value| value != default);
        }
        if !values.is_empty() {
            candidates.push((spec, values));
        }
    }
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    Ok((0..BARS - 1)
        .map(|index| {
            let parameter = if candidates.len() > BARS - 1 {
                index * (candidates.len() - 1) / (BARS - 2)
            } else {
                index % candidates.len()
            };
            let (spec, values) = &candidates[parameter];
            ParameterEdit {
                spec: (*spec).clone(),
                value: values[(parameter + index / candidates.len()) % values.len()].clone(),
            }
        })
        .collect())
}

fn representative_values(spec: &ParameterSpec) -> Result<Vec<ParameterValue>, FullDemoError> {
    Ok(match spec.kind() {
        ParameterKind::Continuous => {
            let range = spec
                .range()
                .ok_or_else(|| failure("missing continuous range"))?;
            let step = spec
                .fine_step()
                .ok_or_else(|| failure("missing continuous step"))?;
            [0.25, 0.5, 0.75]
                .into_iter()
                .map(|fraction| {
                    let value = range.minimum()
                        + ((range.maximum() - range.minimum()) * fraction / step).round() * step;
                    ParameterValue::Continuous(value.clamp(range.minimum(), range.maximum()))
                })
                .collect()
        }
        ParameterKind::Stepped => {
            let range = spec
                .range()
                .ok_or_else(|| failure("missing stepped range"))?;
            [0.0, 0.5, 1.0]
                .into_iter()
                .map(|fraction| {
                    ParameterValue::Stepped(
                        (range.minimum() + (range.maximum() - range.minimum()) * fraction).round()
                            as i64,
                    )
                })
                .collect()
        }
        ParameterKind::Choice => {
            let last = spec
                .choices()
                .len()
                .checked_sub(1)
                .ok_or_else(|| failure("missing parameter choices"))?;
            [0, last / 2, last]
                .into_iter()
                .map(|index| ParameterValue::Choice(spec.choices()[index].id().to_owned()))
                .collect()
        }
        ParameterKind::Toggle => vec![ParameterValue::Toggle(false), ParameterValue::Toggle(true)],
        ParameterKind::Asset => return Err(failure("assets cannot be parameter variations")),
    })
}

#[derive(Default)]
struct DemoPhrase {
    note: usize,
    elapsed: Duration,
    started: bool,
    held: Vec<u8>,
}

impl DemoPhrase {
    fn send<B: ControlAudioBoundary>(
        app: &mut AppLoop<B>,
        note: u8,
        velocity: u8,
    ) -> Result<(), FullDemoError> {
        let message = MidiMessage::try_new(
            MidiChannel::new(0).map_err(failure)?,
            if velocity != 0 {
                MidiMessageKind::NoteOn
            } else {
                MidiMessageKind::NoteOff
            },
            note,
            velocity,
        )
        .map_err(failure)?;
        if let Some(error) = app
            .dispatch_midi_from(message, EventSource::DemoScene)
            .map_err(failure)?
            .boundary_full()
        {
            return Err(failure(error));
        }
        Ok(())
    }

    fn stop<B: ControlAudioBoundary>(&mut self, app: &mut AppLoop<B>) -> Result<(), FullDemoError> {
        for note in self.held.drain(..) {
            Self::send(app, note, 0)?;
        }
        Ok(())
    }

    fn start_notes<B: ControlAudioBoundary>(
        &mut self,
        app: &mut AppLoop<B>,
        notes: Option<&[Vec<(u8, u8)>; NOTES_PER_BAR]>,
    ) -> Result<(), FullDemoError> {
        if let Some(notes) = notes {
            for &(note, velocity) in &notes[self.note % NOTES_PER_BAR] {
                Self::send(app, note, velocity)?;
                self.held.push(note);
            }
        } else {
            let note = ARPEGGIO[self.note % ARPEGGIO.len()];
            Self::send(app, note, 72)?;
            self.held.push(note);
        }
        Ok(())
    }

    /// At most one authored step per tick, with note-off before note-on.
    /// A delayed frame stretches the phrase without replaying overdue steps.
    fn advance<B: ControlAudioBoundary>(
        &mut self,
        app: &mut AppLoop<B>,
        elapsed: Duration,
        notes: Option<&[Vec<(u8, u8)>; NOTES_PER_BAR]>,
    ) -> Result<bool, FullDemoError> {
        if !self.started {
            self.started = true;
            self.start_notes(app, notes)?;
            return Ok(false);
        }
        self.elapsed += elapsed;
        if self.elapsed >= NOTE_GATE {
            self.stop(app)?;
        }
        if self.elapsed < NOTE_LENGTH {
            return Ok(false);
        }
        // Preserve ordinary sub-frame timing, but never replay an overdue
        // interval after a stalled UI tick.
        self.elapsed = if self.elapsed < NOTE_LENGTH * 2 {
            self.elapsed - NOTE_LENGTH
        } else {
            Duration::ZERO
        };
        self.note += 1;
        if self.note.is_multiple_of(NOTES_PER_BAR) {
            self.started = false;
            Ok(true)
        } else {
            self.start_notes(app, notes)?;
            Ok(false)
        }
    }
}

#[derive(Clone, Copy)]
enum Phase {
    Start,
    Play,
    Submit,
    Prepare,
    Activate,
    Complete,
}

pub(crate) struct FullInstrumentEffectDemo {
    plan: FullDemoPlan,
    worker: Box<dyn SessionCandidateWorker>,
    audition: usize,
    bar: usize,
    phase: Phase,
    phrase: DemoPhrase,
    wait: Duration,
    expected_revision: Option<GraphRevision>,
    status: String,
    restore: Vec<ControlEdit>,
    pending_edits: VecDeque<ControlEdit>,
    controls_ready: bool,
}

impl FullInstrumentEffectDemo {
    pub fn check_audio(
        &self,
        snapshot: crate::real_time::AudioObservationSnapshot,
    ) -> Result<(), FullDemoError> {
        if snapshot.routing_failures() != 0
            || snapshot.non_finite_samples() != 0
            || snapshot.voice_limit_refusals() != 0
            || snapshot.active_notes() as usize > self.plan.max_active_notes
        {
            return Err(failure(format!("audio observation: {} routing failures, {} non-finite samples, {} refused notes, {} active notes",
                snapshot.routing_failures(), snapshot.non_finite_samples(), snapshot.voice_limit_refusals(), snapshot.active_notes())));
        }
        Ok(())
    }
    pub fn new(plan: FullDemoPlan, worker: Box<dyn SessionCandidateWorker>) -> Self {
        plan.announce();
        Self {
            plan,
            worker,
            audition: 0,
            bar: 0,
            phase: Phase::Start,
            phrase: DemoPhrase::default(),
            wait: Duration::ZERO,
            expected_revision: None,
            status: "Preparing first audition".into(),
            restore: Vec::new(),
            pending_edits: VecDeque::new(),
            controls_ready: false,
        }
    }

    pub fn document(&self) -> SessionDocumentProjection {
        SessionDocumentProjection::new(
            self.plan.title,
            false,
            if self.is_complete() || (matches!(self.phase, Phase::Play) && self.controls_ready) {
                SessionDocumentMarker::Ready
            } else {
                SessionDocumentMarker::Busy
            },
            None,
            &self.status,
            None,
        )
    }

    pub fn is_complete(&self) -> bool {
        matches!(self.phase, Phase::Complete)
    }

    pub fn shutdown(&mut self) -> Result<(), FullDemoError> {
        self.worker.shutdown_on_control().map_err(failure)?;
        if !self.is_complete() {
            eprintln!(
                "Demo stopped before completion at audition {}/{}: {}",
                self.audition + 1,
                self.plan.auditions.len(),
                self.status
            );
        }
        Ok(())
    }

    fn start_bar(&mut self) -> Result<(), FullDemoError> {
        // Undo the selected parameter before its enabling predicates. This
        // keeps conditional controls reachable while restoring their values.
        self.pending_edits.extend(self.restore.drain(..).rev());
        let audition = &self.plan.auditions[self.audition];
        let pass = &audition.bars[self.bar];
        for edit in pass
            .prerequisites
            .iter()
            .filter(|edit| edit.spec.update() == ParameterUpdate::Scalar)
            .chain(pass.edit.iter())
        {
            let control = match &pass.control {
                PatchControlId::Capability(_) => PatchControlId::Capability(edit.spec.id().clone()),
                PatchControlId::Effect(slot, _) => {
                    PatchControlId::Effect(*slot, edit.spec.id().clone())
                }
                _ => return Err(failure("parameter variation has no parameter control")),
            };
            self.pending_edits.push_back(ControlEdit {
                control,
                edit: edit.clone(),
                remember_original: true,
            });
        }
        self.status = format!(
            "[{}/{}] {} — bar {}/8 — adjusting {}",
            self.audition + 1,
            self.plan.auditions.len(),
            audition.label,
            self.bar + 1,
            pass.label
        );
        self.controls_ready = false;
        self.wait = Duration::ZERO;
        self.phase = Phase::Play;
        Ok(())
    }

    fn advance_controls<B: ControlAudioBoundary>(
        &mut self,
        app: &mut AppLoop<B>,
    ) -> Result<(), FullDemoError> {
        let started = Instant::now();
        while !self.controls_ready && started.elapsed() < CONTROL_WORK_PER_TICK {
            if let Some(next) = self.pending_edits.front_mut() {
                if next.remember_original {
                    self.restore.push(ControlEdit {
                        control: next.control.clone(),
                        edit: ParameterEdit {
                            spec: next.edit.spec.clone(),
                            value: current_value(app, &next.control)?.clone(),
                        },
                        remember_original: false,
                    });
                    next.remember_original = false;
                }
                if let Some(action) = control_action(app, &next.control, Some(&next.edit))? {
                    dispatch(app, action)?;
                } else {
                    self.pending_edits.pop_front();
                }
            } else {
                let audition = &self.plan.auditions[self.audition];
                let pass = &audition.bars[self.bar];
                if let Some(action) = control_action(app, &pass.control, None)? {
                    dispatch(app, action)?;
                    continue;
                }
                self.status = format!(
                    "[{}/{}] {} — bar {}/8 — {}{}",
                    self.audition + 1,
                    self.plan.auditions.len(),
                    audition.label,
                    self.bar + 1,
                    pass.label,
                    if let Some(edit) = &pass.edit {
                        format!(
                            " — {}",
                            display_value(&edit.spec, current_value(app, &pass.control)?)
                        )
                    } else {
                        String::new()
                    }
                );
                eprintln!("{}", self.status);
                self.controls_ready = true;
            }
        }
        Ok(())
    }

    pub fn advance<B: ControlAudioBoundary>(
        &mut self,
        app: &mut AppLoop<B>,
        elapsed: Duration,
    ) -> Result<bool, FullDemoError> {
        app.advance_structural().map_err(failure)?;
        if !matches!(self.phase, Phase::Play | Phase::Complete) {
            self.wait += elapsed;
            if self.wait > PREPARATION_TIMEOUT {
                return Err(failure(format!("timed out: {}", self.status)));
            }
        }
        match self.phase {
            Phase::Start => self.start_bar()?,
            Phase::Play => {
                if self.phrase.advance(
                    app,
                    elapsed,
                    self.plan.auditions[self.audition].bars[self.bar]
                        .notes
                        .as_ref(),
                )? {
                    if !self.controls_ready {
                        return Err(failure(format!(
                            "parameter adjustment overran its bar: {}",
                            self.status
                        )));
                    }
                    self.bar += 1;
                    let audition = &self.plan.auditions[self.audition];
                    if self.bar == BARS
                        || audition.bars[self.bar].requires_preparation
                        || audition.bars[self.bar - 1].requires_preparation
                    {
                        self.phase = Phase::Submit;
                    } else {
                        // Keep MIDI on the same clock across scalar changes.
                        // The next note starts before control-side navigation;
                        // the existing engine and effect retain their state.
                        self.phrase.advance(
                            app,
                            Duration::ZERO,
                            self.plan.auditions[self.audition].bars[self.bar]
                                .notes
                                .as_ref(),
                        )?;
                        self.start_bar()?;
                    }
                }
            }
            Phase::Submit => {
                self.phrase.stop(app)?;
                self.restore.clear();
                self.pending_edits.clear();
                self.phrase.elapsed = Duration::ZERO;
                if self.bar == BARS {
                    self.audition += 1;
                    self.bar = 0;
                    self.phrase = DemoPhrase::default();
                }
                if self.audition == self.plan.auditions.len() {
                    self.phase = Phase::Complete;
                    self.status = format!("{} complete", self.plan.title);
                    eprintln!(
                        "{}: {} instruments, {} effects, {} auditions.",
                        self.status,
                        self.plan.instruments,
                        self.plan.effects,
                        self.plan.auditions.len()
                    );
                    return Ok(false);
                }
                let revision = app.next_structural_graph_revision().map_err(failure)?;
                let audition = &self.plan.auditions[self.audition];
                let pass = &audition.bars[self.bar];
                self.status = format!(
                    "Preparing [{}/{}] {} — bar {}/8 — {}",
                    self.audition + 1,
                    self.plan.auditions.len(),
                    audition.label,
                    self.bar + 1,
                    pass.label
                );
                eprintln!("{}", self.status);
                self.worker
                    .try_submit(SessionCandidateRequest::new(
                        SessionCandidateToken::new((self.audition * BARS + self.bar) as u64 + 1)
                            .map_err(failure)?,
                        SessionCandidateSource::Default(Box::new(pass.session.clone())),
                        revision,
                    ))
                    .map_err(|error| failure(format!("session worker: {:?}", error.reason())))?;
                self.expected_revision = Some(revision);
                self.phase = Phase::Prepare;
                self.wait = Duration::ZERO;
            }
            Phase::Prepare => {
                if let Some(result) = self.worker.try_poll() {
                    if result.token().value() != (self.audition * BARS + self.bar) as u64 + 1 {
                        return Err(failure("unexpected preparation token"));
                    }
                    match result {
                        SessionCandidateResult::Prepared { prepared, .. } => {
                            let (replacement, graph) = prepared.into_replacement();
                            if Some(graph.revision()) != self.expected_revision {
                                return Err(failure("unexpected prepared graph revision"));
                            }
                            app.stage_session_replacement(replacement, graph)
                                .map_err(failure)?;
                            self.phase = Phase::Activate;
                        }
                        SessionCandidateResult::Failed { failure: error, .. } => {
                            return Err(failure(format!("{}: {error}", self.status)))
                        }
                    }
                }
            }
            Phase::Activate => {
                if !app.session_replacement_pending()
                    && Some(app.current_parameters().graph_revision()) == self.expected_revision
                {
                    self.phase = Phase::Start;
                    self.wait = Duration::ZERO;
                }
            }
            Phase::Complete => return Ok(false),
        }
        if matches!(self.phase, Phase::Play) {
            self.advance_controls(app)?;
        }
        Ok(true)
    }
}

fn control_action<B: ControlAudioBoundary>(
    app: &AppLoop<B>,
    control: &PatchControlId,
    edit: Option<&ParameterEdit>,
) -> Result<Option<SemanticAction>, FullDemoError> {
    if let Some(action) = next_patch_control_action(app.state(), control).map_err(failure)? {
        return Ok(Some(action));
    }
    if app.state().interaction().mode() != InteractionMode::Adjust {
        return Ok(Some(SemanticAction::SetInteractionMode(
            InteractionMode::Adjust,
        )));
    }
    match edit {
        Some(edit) => Ok(
            adjustment_toward(&edit.spec, current_value(app, control)?, &edit.value)?
                .map(SemanticAction::Adjust),
        ),
        None => Ok(None),
    }
}

fn dispatch<B: ControlAudioBoundary>(
    app: &mut AppLoop<B>,
    action: SemanticAction,
) -> Result<(), FullDemoError> {
    if let Some(error) = app
        .dispatch_action_from(action, EventSource::DemoScene)
        .map_err(failure)?
        .boundary_full()
    {
        return Err(failure(error));
    }
    Ok(())
}

fn display_value(spec: &ParameterSpec, value: &ParameterValue) -> String {
    if let Some(label) = spec.value_label(value) {
        return label.into_owned();
    }
    let value = match value {
        ParameterValue::Continuous(value) => value.to_string(),
        ParameterValue::Stepped(value) => value.to_string(),
        ParameterValue::Choice(id) => spec
            .choices()
            .iter()
            .find(|choice| choice.id() == id)
            .map(|choice| choice.label())
            .unwrap_or(id)
            .to_owned(),
        ParameterValue::Toggle(value) => if *value { "On" } else { "Off" }.to_owned(),
    };
    match spec.unit() {
        Some(unit) => format!("{value} {unit}"),
        None => value,
    }
}

fn current_value<'a, B: ControlAudioBoundary>(
    app: &'a AppLoop<B>,
    control: &PatchControlId,
) -> Result<&'a ParameterValue, FullDemoError> {
    let patch = app
        .patches()
        .first()
        .ok_or_else(|| failure("missing audition Patch"))?;
    match control {
        PatchControlId::Capability(id) => patch.instrument_config().value(id),
        PatchControlId::Effect(slot, id) => patch
            .effect_slots()
            .iter()
            .flatten()
            .find(|effect| effect.slot_id() == *slot)
            .and_then(|effect| effect.value(id)),
        _ => None,
    }
    .ok_or_else(|| failure("missing audition parameter"))
}

fn adjustment_toward(
    spec: &ParameterSpec,
    current: &ParameterValue,
    target: &ParameterValue,
) -> Result<Option<Direction>, FullDemoError> {
    let target = f64::from(spec.scalar_value(target).map_err(failure)?);
    let current_number = f64::from(spec.scalar_value(current).map_err(failure)?);
    let mut distance = (target - current_number).abs();
    let mut best = None;
    // Choose the largest ordinary gesture that gets strictly closer. This
    // terminates even when f32 projection or quantization cannot hit a value
    // exactly; the report prints the actual canonical value, never the target.
    for (direction, adjustment) in [
        (Direction::Left, ParameterAdjustment::FineDecrease),
        (Direction::Right, ParameterAdjustment::FineIncrease),
        (Direction::Down, ParameterAdjustment::CoarseDecrease),
        (Direction::Up, ParameterAdjustment::CoarseIncrease),
    ] {
        if spec.kind() == ParameterKind::Choice
            && matches!(direction, Direction::Up | Direction::Down)
        {
            continue;
        }
        let value = match spec.adjusted_scalar_value(current, adjustment) {
            Ok(value) => value,
            Err(crate::synth::CapabilityError::ScalarValueAtBoundary(_)) => continue,
            Err(error) => return Err(failure(error)),
        };
        let next_distance = (target - f64::from(spec.scalar_value(&value).map_err(failure)?)).abs();
        if next_distance < distance {
            distance = next_distance;
            best = Some(direction);
        }
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::atomic_audio_observation::{
        AtomicAudioObservation, AtomicAudioObservationReader, AtomicAudioObservationWriter,
    };
    use crate::adapter::braids_capability::BRAIDS_CAPABILITY_ID;
    use crate::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
    use crate::adapter::lock_free_structural_graph_boundary::LockFreeStructuralGraphBoundary;
    use crate::adapter::production_effects::{
        production_effect_preparers, production_effect_registry,
    };
    use crate::adapter::production_instruments::{
        production_capability_registry, production_instrument_preparers,
        production_instrument_providers,
    };
    use crate::adapter::threaded_graph_preparation_worker::ThreadedGraphPreparationWorker;
    use crate::adapter::threaded_session_candidate_worker::ThreadedSessionCandidateWorker;
    use crate::control::StateProjector;
    use crate::real_time::{
        AudioBoundary, AudioObservation, AudioRenderer, ControlAudioObservation,
        GraphHandoffStatus, ParameterSnapshot, StructuralGraphBoundary,
    };
    use crate::shell::audio_output::{AudioDeviceConfig, AudioSampleFormat};
    use std::collections::BTreeSet;
    use std::time::Instant;

    type Control = <LockFreeAudioBoundary as AudioBoundary>::ControlHandle;
    type Renderer = AudioRenderer<
        <LockFreeAudioBoundary as AudioBoundary>::AudioHandle,
        <LockFreeStructuralGraphBoundary as StructuralGraphBoundary>::AudioHandle,
        AtomicAudioObservationWriter,
    >;

    fn plan() -> (
        FullDemoPlan,
        DescriptorDefaultConfigFactory,
        EffectCapabilityRegistry,
    ) {
        let factory = DescriptorDefaultConfigFactory::new(
            production_capability_registry().unwrap(),
            production_instrument_providers().unwrap(),
        );
        let effects = production_effect_registry().unwrap();
        let plan = FullDemoPlan::build(&factory, &effects, &selection()).unwrap();
        (plan, factory, effects)
    }

    struct Harness {
        app: AppLoop<Control>,
        renderer: Renderer,
        observation: AtomicAudioObservationReader,
        demo: FullInstrumentEffectDemo,
    }

    impl Harness {
        fn new(
            plan: FullDemoPlan,
            factory: DescriptorDefaultConfigFactory,
            effects: EffectCapabilityRegistry,
        ) -> Self {
            Self::with_preparers(plan, factory, effects, || {
                production_instrument_preparers().unwrap()
            })
        }

        fn with_preparers(
            plan: FullDemoPlan,
            factory: DescriptorDefaultConfigFactory,
            effects: EffectCapabilityRegistry,
            preparers: impl Fn() -> Vec<Box<dyn crate::synth::InstrumentPreparer>>,
        ) -> Self {
            let config = AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 256).unwrap();
            let registry = factory.registry().clone();
            let initial = plan
                .initial_session()
                .prepare_restore(
                    registry.clone(),
                    effects.clone(),
                    &preparers(),
                    &production_effect_preparers().unwrap(),
                    GraphRevision::INITIAL,
                    48_000.0,
                    256,
                )
                .unwrap();
            let (replacement, graph) = initial.into_replacement();
            let mut state = AppState::for_graph_with_effects(
                registry.clone(),
                effects.clone(),
                GlobalParameters::new(-18.0).unwrap(),
                GraphRevision::INITIAL,
            )
            .with_patch_creation_blueprint(
                crate::control::PatchCreationBlueprint::resolve(
                    &CapabilityId::new(crate::adapter::sample_capability::SAMPLE_CAPABILITY_ID)
                        .unwrap(),
                    &factory,
                )
                .unwrap(),
            );
            if let Some(listing) =
                crate::adapter::production_instruments::production_sample_root_listing().unwrap()
            {
                state = state.with_sample_catalog([listing]);
            }
            state
                .apply(AppEvent::ReplacePersistedSession(Box::new(replacement)))
                .unwrap();
            let (control, audio) = LockFreeAudioBoundary::new(
                1024,
                ParameterSnapshot::new(0, *state.global(), state.mixer().clone(), &[]).unwrap(),
            )
            .into_handles();
            let (structural_control, structural_audio) = LockFreeStructuralGraphBoundary::new(
                1,
                1,
                GraphHandoffStatus::with_active(GraphRevision::INITIAL),
            )
            .unwrap()
            .into_handles();
            let mut app = AppLoop::new(
                state,
                StateProjector::for_graph(GraphRevision::INITIAL),
                control,
            )
            .unwrap();
            let worker = ThreadedGraphPreparationWorker::new_with_effects(
                registry.clone(),
                preparers(),
                effects.clone(),
                production_effect_preparers().unwrap(),
                config,
            )
            .unwrap();
            app.configure_engine_selection(factory, worker, structural_control, &graph, config)
                .unwrap();
            let candidates = ThreadedSessionCandidateWorker::new(
                registry,
                preparers(),
                effects,
                production_effect_preparers().unwrap(),
                config,
            )
            .unwrap();
            let (writer, observation) = AtomicAudioObservation::default().into_handles();
            Self {
                app,
                renderer: AudioRenderer::with_observation(audio, structural_audio, graph, writer),
                observation,
                demo: FullInstrumentEffectDemo::new(plan, Box::new(candidates)),
            }
        }

        fn render(&mut self) -> f32 {
            let mut output = [0.0f32; 512];
            self.renderer.render(&mut output);
            assert!(
                output.iter().all(|sample| sample.is_finite()),
                "{}",
                self.demo.status
            );
            self.demo
                .check_audio(self.observation.read_latest_on_control())
                .unwrap_or_else(|error| panic!("{}: {error}", self.demo.status));
            output.iter().map(|sample| sample.abs()).fold(0.0, f32::max)
        }

        fn shutdown(mut self) {
            self.demo.phrase.stop(&mut self.app).unwrap();
            self.render();
            assert_eq!(self.observation.read_latest_on_control().active_notes(), 0);
            drop(self.renderer);
            self.demo.shutdown().unwrap();
            self.app.shutdown_engine_selection_on_control().unwrap();
        }
    }

    fn selection() -> FullDemoSelection {
        FullDemoSelection {
            effect_reference: CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap(),
            skipped_instruments: [
                crate::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID,
                BRAIDS_CAPABILITY_ID,
                crate::adapter::drum_rack_capability::DRUM_RACK_CAPABILITY_ID,
            ]
            .into_iter()
            .map(|id| CapabilityId::new(id).unwrap())
            .collect(),
            skipped_effects: [
                crate::adapter::chorus_capability::CHORUS_CAPABILITY_ID,
                crate::adapter::reverb_capability::REVERB_CAPABILITY_ID,
                crate::adapter::delay_capability::DELAY_CAPABILITY_ID,
            ]
            .into_iter()
            .map(|id| EffectCapabilityId::new(id).unwrap())
            .collect(),
        }
    }

    #[test]
    fn drum_rack_demo_plays_every_pad_and_layered_groove_through_production() {
        use crate::adapter::{
            drum_rack_capability::{DrumRackCapability, DRUM_RACK_PAD_PARAMETER_ID},
            drum_rack_preparer::DrumRackPreparer,
            filesystem_sample_catalog::FilesystemSampleCatalog,
            sample_capability::SampleCapability,
            sample_preparer::SamplePreparer,
            wav_sample_decoder::WavSampleDecoder,
        };
        use crate::synth::{AssetFileId, CapabilityRegistry, InstrumentCapabilityProvider};
        use std::sync::Arc;
        struct Fixture(std::path::PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = Fixture(std::env::temp_dir().join(format!(
            "crest-drum-demo-test-{}-{unique}",
            std::process::id()
        )));
        std::fs::create_dir(&root.0).unwrap();
        crate::testing::drum_rack_demo::write_samples(&root.0).unwrap();
        let initial = AssetFileId::new(crate::testing::drum_rack_demo::sample_filename(0)).unwrap();
        let sample = SampleCapability::new(initial.clone()).unwrap();
        let drum = DrumRackCapability::new(initial.clone()).unwrap();
        let registry =
            CapabilityRegistry::new(vec![sample.descriptor(), drum.descriptor()]).unwrap();
        let factory =
            DescriptorDefaultConfigFactory::new(registry, vec![Box::new(sample), Box::new(drum)]);
        let effects = production_effect_registry().unwrap();
        let plan = crate::testing::drum_rack_demo::build_plan(&factory, &effects).unwrap();
        let catalog = Arc::new(FilesystemSampleCatalog::new(&root.0).unwrap());
        let mut harness = Harness::with_preparers(plan, factory, effects, || {
            let sample = SamplePreparer::new(catalog.clone(), Arc::new(WavSampleDecoder)).unwrap();
            let drum = DrumRackPreparer::with_sample(initial.clone(), sample.clone()).unwrap();
            vec![Box::new(sample), Box::new(drum)]
        });
        assert_eq!(
            harness.app.patches()[0]
                .instrument_config()
                .asset_references()
                .len(),
            16
        );
        let mut visited = BTreeSet::new();
        let mut sounded = BTreeSet::new();
        let mut polyphonic = false;
        let mut selected = BTreeSet::new();
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            assert!(Instant::now() < deadline, "{}", harness.demo.status);
            let running = harness
                .demo
                .advance(&mut harness.app, Duration::from_millis(16))
                .unwrap();
            let mut peak = 0.0_f32;
            for _ in 0..3 {
                peak = peak.max(harness.render());
            }
            if matches!(harness.demo.phase, Phase::Play) {
                let position = (harness.demo.audition, harness.demo.bar);
                visited.insert(position);
                if peak > 0.0001 {
                    sounded.insert(position);
                }
                polyphonic |= harness.observation.read_latest_on_control().active_notes() >= 3;
                if harness.demo.controls_ready {
                    assert_eq!(
                        harness.app.state().interaction().active_surface(),
                        crate::control::SurfaceId::PatchDetail
                    );
                    let pass = &harness.demo.plan.auditions[position.0].bars[position.1];
                    assert_eq!(
                        current_value(&harness.app, &pass.control).unwrap(),
                        &pass.edit.as_ref().unwrap().value
                    );
                    let value = harness.app.patches()[0]
                        .instrument_config()
                        .value(&crate::synth::ParameterId::new(DRUM_RACK_PAD_PARAMETER_ID).unwrap())
                        .unwrap();
                    selected.insert(format!("{value:?}"));
                }
            }
            if !running {
                break;
            }
            if matches!(harness.demo.phase, Phase::Prepare) {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        assert!(harness.demo.is_complete());
        assert_eq!(visited.len(), 24);
        assert_eq!(
            sounded, visited,
            "every pad and groove bar must produce audio"
        );
        assert_eq!(selected.len(), 16);
        assert!(polyphonic, "groove must mix simultaneous pads");
        harness.shutdown();
    }

    #[test]
    fn full_demo_plan_skips_known_entries_and_gives_each_new_entry_eight_bars() {
        let (plan, factory, effects) = plan();
        let selection = selection();
        let braids = factory.create(&selection.effect_reference).unwrap();
        let expected_braids = serde_json::to_value(&braids).unwrap();
        let mut dry = BTreeSet::new();
        let mut wet = BTreeSet::new();
        let mut effects_started = false;
        for audition in &plan.auditions {
            assert_eq!(audition.bars.len(), BARS);
            assert!(audition.bars[0].edit.is_none());
            assert!(matches!(
                audition.bars[0].control,
                PatchControlId::Engine | PatchControlId::EffectSlot(_)
            ));
            for pass in &audition.bars {
                let saved = serde_json::to_value(&pass.session).unwrap();
                let patches = saved["patches"].as_array().unwrap();
                assert_eq!(patches.len(), 1);
                let patch = &patches[0];
                assert_eq!(patch["voiceLimit"], 1);
                assert_eq!(patch["channel"], 0);
                assert!(saved["returns"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|ret| ret["effect"].is_null()));
                let occupants = patch["effects"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|effect| !effect.is_null())
                    .collect::<Vec<_>>();
                if occupants.is_empty() {
                    assert!(
                        !effects_started,
                        "dry instruments must precede every effect"
                    );
                    dry.insert(
                        patch["instrument"]["capabilityId"]
                            .as_str()
                            .unwrap()
                            .to_owned(),
                    );
                } else {
                    effects_started = true;
                    assert_eq!(occupants.len(), 1);
                    assert_eq!(patch["instrument"], expected_braids);
                    wet.insert(occupants[0]["capabilityId"].as_str().unwrap().to_owned());
                }
                if let Some(edit) = &pass.edit {
                    edit.spec.scalar_value(&edit.value).unwrap();
                    assert_ne!(
                        edit.spec.default_value(),
                        &ParameterDefault::Value(edit.value.clone())
                    );
                }
            }
        }
        let expected_dry = factory
            .registry()
            .descriptors()
            .iter()
            .filter(|descriptor| !selection.skipped_instruments.contains(descriptor.id()))
            .map(|descriptor| descriptor.id().as_str().to_owned())
            .collect::<BTreeSet<_>>();
        let expected_wet = effects
            .descriptors()
            .iter()
            .filter(|descriptor| !selection.skipped_effects.contains(descriptor.id()))
            .map(|descriptor| descriptor.id().as_str().to_owned())
            .collect::<BTreeSet<_>>();
        assert_eq!(dry, expected_dry);
        assert_eq!(wet, expected_wet);
        assert!(dry.contains(crate::adapter::sample_capability::SAMPLE_CAPABILITY_ID));
        assert_eq!(plan.instruments, dry.len());
        assert_eq!(plan.effects, wet.len());
        assert_eq!(plan.auditions.len(), dry.len() + wet.len());
        eprintln!(
            "Catalog tour: {} instruments, {} effects, {:.1} minutes plus preparation",
            plan.instruments,
            plan.effects,
            plan.auditions.len() as f64 * 16.0 / 60.0
        );
    }

    #[test]
    fn full_demo_variations_sample_parameters_without_exhaustive_expansion() {
        let (_, factory, effects) = plan();
        for specs in factory
            .registry()
            .descriptors()
            .iter()
            .map(|descriptor| descriptor.parameters().cloned().collect::<Vec<_>>())
            .chain(
                effects
                    .descriptors()
                    .iter()
                    .map(|descriptor| descriptor.parameters().cloned().collect::<Vec<_>>()),
            )
        {
            let eligible = specs
                .iter()
                .filter(|spec| {
                    spec.kind() != ParameterKind::Asset
                        && spec.patch_interaction() != PatchInteraction::ReadOnly
                        && representative_values(spec).unwrap().iter().any(|value| {
                            spec.default_value() != &ParameterDefault::Value(value.clone())
                        })
                })
                .map(|spec| spec.id())
                .collect::<Vec<_>>();
            let edits = representative_edits(&specs).unwrap();
            if eligible.is_empty() {
                assert!(edits.is_empty());
            } else {
                assert_eq!(edits.len(), BARS - 1);
                let covered = edits
                    .iter()
                    .map(|edit| edit.spec.id())
                    .collect::<BTreeSet<_>>();
                assert_eq!(covered.len(), eligible.len().min(BARS - 1));
                assert!(covered.contains(eligible[0]));
                assert!(covered.contains(eligible[eligible.len() - 1]));
                for edit in edits {
                    if edit.spec.update() == ParameterUpdate::Scalar {
                        edit.spec.scalar_value(&edit.value).unwrap();
                    } else if let ParameterValue::Choice(choice) = &edit.value {
                        assert!(edit.spec.choices().iter().any(|item| item.id() == choice));
                    } else {
                        panic!("unsupported structural variation: {}", edit.spec.id());
                    }
                }
            }
        }
    }

    #[test]
    fn full_demo_arpeggio_is_monophonic_and_eight_complete_bars() {
        let (_, factory, effects) = plan();
        let plan = FullDemoPlan::build(
            &factory,
            &effects,
            &FullDemoSelection {
                effect_reference: CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap(),
                skipped_instruments: factory
                    .registry()
                    .descriptors()
                    .iter()
                    .filter(|descriptor| descriptor.id().as_str() != BRAIDS_CAPABILITY_ID)
                    .map(|descriptor| descriptor.id().clone())
                    .collect(),
                skipped_effects: effects
                    .descriptors()
                    .iter()
                    .map(|descriptor| descriptor.id().clone())
                    .collect(),
            },
        )
        .unwrap();
        assert_eq!(plan.auditions.len(), 1);
        let mut harness = Harness::new(plan, factory, effects);
        let mut arp = DemoPhrase::default();
        let mut bars = 0;
        let mut musical_time = Duration::ZERO;
        let mut peak = 0.0f32;
        for _ in 0..BARS {
            assert!(!arp.advance(&mut harness.app, Duration::ZERO, None).unwrap());
            peak = peak.max(harness.render());
            for _ in 0..NOTES_PER_BAR {
                assert!(!arp.advance(&mut harness.app, NOTE_GATE, None).unwrap());
                harness.render();
                let observation = harness.observation.read_latest_on_control();
                assert_eq!(observation.active_notes(), 0);
                musical_time += NOTE_GATE;
                if arp
                    .advance(&mut harness.app, NOTE_LENGTH - NOTE_GATE, None)
                    .unwrap()
                {
                    bars += 1;
                }
                musical_time += NOTE_LENGTH - NOTE_GATE;
                peak = peak.max(harness.render());
            }
        }
        assert_eq!(bars, BARS);
        assert_eq!(arp.note, BARS * NOTES_PER_BAR);
        assert_eq!(musical_time, Duration::from_secs(16));
        assert!(peak > 0.0001, "production Braids must sound");
        assert!(arp.held.is_empty());
        // A stalled tick advances only one note, never replays missed notes.
        arp.advance(&mut harness.app, Duration::ZERO, None).unwrap();
        harness.render();
        let before = arp.note;
        arp.advance(&mut harness.app, Duration::from_secs(5), None)
            .unwrap();
        harness.render();
        assert_eq!(arp.note, before + 1);
        harness.demo.phrase = arp;
        harness.shutdown();
    }

    #[test]
    fn full_demo_waits_without_notes_and_surfaces_candidate_failure() {
        struct DelayedFailure {
            result: Option<SessionCandidateResult>,
            polls: usize,
        }
        impl SessionCandidateWorker for DelayedFailure {
            fn try_submit(
                &mut self,
                request: SessionCandidateRequest,
            ) -> Result<(), crate::control::SessionCandidateWorkerBusy> {
                self.result = Some(SessionCandidateResult::Failed {
                    token: request.token(),
                    failure: crate::control::SessionCandidateFailure::DecodeEncoding,
                });
                Ok(())
            }
            fn try_poll(&mut self) -> Option<SessionCandidateResult> {
                self.polls += 1;
                if self.polls <= 2 {
                    None
                } else {
                    self.result.take()
                }
            }
            fn shutdown_on_control(&mut self) -> Result<(), crate::real_time::WorkerShutdownError> {
                Ok(())
            }
        }
        let (mut plan, factory, effects) = plan();
        plan.auditions.truncate(2);
        let mut harness = Harness::new(plan, factory, effects);
        harness.demo.worker.shutdown_on_control().unwrap();
        harness.demo.worker = Box::new(DelayedFailure {
            result: None,
            polls: 0,
        });
        harness.demo.phase = Phase::Submit;
        harness.demo.bar = BARS;
        harness
            .demo
            .advance(&mut harness.app, Duration::ZERO)
            .unwrap();
        for _ in 0..2 {
            harness
                .demo
                .advance(&mut harness.app, Duration::from_secs(1))
                .unwrap();
            harness.render();
            assert!(matches!(harness.demo.phase, Phase::Prepare));
            assert_eq!(harness.demo.phrase.note, 0);
            assert_eq!(
                harness.observation.read_latest_on_control().active_notes(),
                0
            );
        }
        let error = harness
            .demo
            .advance(&mut harness.app, Duration::ZERO)
            .unwrap_err();
        assert!(error.to_string().contains("not UTF-8"));
        assert!(!harness.demo.is_complete());
        harness.shutdown();
    }

    #[test]
    #[cfg_attr(debug_assertions, ignore = "production timing requires --release")]
    fn full_demo_live_control_cost_does_not_overrun_parameter_bars() {
        let (mut plan, factory, effects) = plan();
        // The first Sample variation reproduces the physical-window failure.
        plan.auditions.truncate(1);
        let mut harness = Harness::new(plan, factory, effects);
        let tick = Duration::from_millis(16);
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut last_tick = Instant::now();
        let mut longest_control_tick = Duration::ZERO;
        while harness.demo.bar < 2 {
            assert!(Instant::now() < deadline, "{}", harness.demo.status);
            let now = Instant::now();
            // Include real reducer/projection cost in the musical clock. A
            // fixed simulated 16 ms tick hid a 45 ms production action cost.
            let elapsed = now.duration_since(last_tick).max(tick);
            last_tick = now;
            harness.demo.advance(&mut harness.app, elapsed).unwrap();
            longest_control_tick = longest_control_tick.max(now.elapsed());
            for _ in 0..3 {
                harness.render();
            }
        }
        eprintln!(
            "Production catalog with elapsed control cost: longest tick {longest_control_tick:?}"
        );
        harness.shutdown();
    }

    #[test]
    fn full_demo_scalar_bars_keep_the_graph_and_regular_note_spacing() {
        let (mut plan, factory, effects) = plan();
        let effect = plan.auditions.remove(plan.instruments);
        plan.auditions = vec![effect];
        let mut harness = Harness::new(plan, factory, effects);
        let tick = Duration::from_millis(16);
        let mut clock = Duration::ZERO;
        let mut note_starts = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            assert!(Instant::now() < deadline, "{}", harness.demo.status);
            let previous = (harness.demo.phrase.note, harness.demo.phrase.held.clone());
            let running = harness.demo.advance(&mut harness.app, tick).unwrap();
            let current = (harness.demo.phrase.note, harness.demo.phrase.held.clone());
            if !current.1.is_empty() && current != previous {
                note_starts.push(clock);
            }
            for _ in 0..3 {
                harness.render();
            }
            assert_eq!(
                harness.app.current_parameters().graph_revision(),
                GraphRevision::INITIAL,
                "scalar parameter changes must preserve the playing engine and effect tails"
            );
            clock += tick;
            if !running {
                break;
            }
            if matches!(harness.demo.phase, Phase::Prepare) {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        assert_eq!(note_starts.len(), BARS * NOTES_PER_BAR);
        let longest = note_starts
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .max()
            .unwrap();
        assert!(longest <= NOTE_LENGTH + tick, "arpeggio gap: {longest:?}");
        harness.shutdown();
    }

    #[test]
    #[cfg_attr(debug_assertions, ignore = "production timing requires --release")]
    fn full_demo_all_auditions_cross_worker_reducer_projection_and_native_render() {
        let (plan, factory, effects) = plan();
        let mut harness = Harness::new(plan, factory, effects);
        let deadline = Instant::now() + Duration::from_secs(900);
        let tick = Duration::from_millis(16);
        let mut clock = Duration::ZERO;
        let mut checked_bars = BTreeSet::new();
        let mut sounding_defaults = BTreeSet::new();
        let mut note_counts = vec![0; harness.demo.plan.auditions.len()];
        let mut previous_start = None;
        let mut entry_revision = GraphRevision::INITIAL;
        let mut longest_control_tick = Duration::ZERO;
        loop {
            assert!(
                Instant::now() < deadline,
                "tour stalled: {}",
                harness.demo.status
            );
            let previous = (
                harness.demo.audition,
                harness.demo.phrase.note,
                harness.demo.phrase.held.clone(),
            );
            let started = Instant::now();
            let running = harness
                .demo
                .advance(&mut harness.app, tick)
                .unwrap_or_else(|error| panic!("{}: {error}", harness.demo.status));
            if matches!(harness.demo.phase, Phase::Play) {
                longest_control_tick = longest_control_tick.max(started.elapsed());
            }
            let current = (
                harness.demo.audition,
                harness.demo.phrase.note,
                harness.demo.phrase.held.clone(),
            );
            if !current.2.is_empty() && current != previous {
                note_counts[current.0] += 1;
                if let Some((entry, last)) = previous_start {
                    if entry == current.0 {
                        assert!(
                            clock - last <= NOTE_LENGTH + tick,
                            "arpeggio stalled: {}",
                            harness.demo.status
                        );
                    }
                }
                previous_start = Some((current.0, clock));
            }
            let mut peak = 0.0f32;
            for _ in 0..3 {
                peak = peak.max(harness.render());
            }
            if matches!(harness.demo.phase, Phase::Play) {
                assert_eq!(harness.app.patches().len(), 1);
                assert_eq!(harness.app.patches()[0].voice_limit().value(), 1);
                if harness.demo.bar == 0 {
                    entry_revision = harness.app.current_parameters().graph_revision();
                    if peak > 0.00001 {
                        sounding_defaults.insert(harness.demo.audition);
                    }
                }
                assert_eq!(
                    harness.app.current_parameters().graph_revision(),
                    entry_revision,
                    "scalar edits must preserve the active graph"
                );
                if harness.demo.controls_ready
                    && checked_bars.insert((harness.demo.audition, harness.demo.bar))
                {
                    let pass =
                        &harness.demo.plan.auditions[harness.demo.audition].bars[harness.demo.bar];
                    if let Some(edit) = &pass.edit {
                        let value = current_value(&harness.app, &pass.control).unwrap();
                        assert!(adjustment_toward(&edit.spec, value, &edit.value)
                            .unwrap()
                            .is_none());
                    }
                }
            } else {
                assert!(harness.demo.phrase.held.is_empty());
                assert_eq!(
                    harness.observation.read_latest_on_control().active_notes(),
                    0
                );
            }
            if !running {
                break;
            }
            if matches!(harness.demo.phase, Phase::Prepare) {
                std::thread::sleep(Duration::from_millis(1));
            }
            clock += tick;
        }
        assert!(harness.demo.is_complete());
        assert_eq!(checked_bars.len(), harness.demo.plan.auditions.len() * BARS);
        for (index, audition) in harness.demo.plan.auditions.iter().enumerate() {
            assert_eq!(
                note_counts[index],
                BARS * NOTES_PER_BAR,
                "{}",
                audition.label
            );
            assert!(
                sounding_defaults.contains(&index),
                "silent default: {}",
                audition.label
            );
        }
        eprintln!("All {} entries played every note and reached every planned bar target; longest measured playing control tick: {longest_control_tick:?}", note_counts.len());
        harness.shutdown();
    }
}
