//! Optional MPE capture owner. Packet plans clone decoder ownership/cache only;
//! accepted curve history is appended in place and copied once when a note ends.
use super::*;
use crate::mpe::{MpeDecoder, MpeDecoderCheckpoint, MpeProvenance};

const POINTS_PER_PACKET: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PhysicalRelease {
    at: Beat,
    velocity: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Builder {
    voice: u64,
    id: EventId,
    at: Beat,
    target: Target,
    pitch: u8,
    velocity: u8,
    provenance: MpeProvenance,
    release: Option<PhysicalRelease>,
    member: Vec<Expression>,
    master: Vec<Expression>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MpeCaptureCheckpoint {
    decoder: MpeDecoderCheckpoint,
    pending: Vec<Builder>,
    next_sequence: u64,
    points: usize,
}

#[derive(Debug, Clone)]
pub(super) struct MpeCapture {
    decoder: MpeDecoder,
    pending: BTreeMap<u64, Builder>,
    next_sequence: u64,
    points: usize,
    monitor: Vec<MpeEvent>,
    monitor_units: usize,
    limit: usize,
}

#[derive(Default)]
struct Change {
    new: Option<Builder>,
    release: Option<PhysicalRelease>,
    member: Vec<Expression>,
    master: Vec<Expression>,
    ended: bool,
}

pub(super) struct Stage {
    decoder: MpeDecoder,
    changes: BTreeMap<u64, Change>,
    completed: Vec<Event>,
    next_identity: u64,
    next_sequence: u64,
    points: usize,
    monitor: Vec<MpeEvent>,
    monitor_units: usize,
}

impl MpeCapture {
    pub(super) fn shift_event_identities(&mut self, shift: &edit_guard::CaptureIdentityShift) {
        for builder in self.pending.values_mut() {
            builder.id = shift.event(builder.id);
        }
    }
    pub(super) fn new(profile: MpeProfile, capacity: usize) -> Result<Self, RecordingError> {
        let limit = point_limit(capacity)?;
        Ok(Self {
            decoder: MpeDecoder::new(profile, capacity)?,
            pending: BTreeMap::new(),
            next_sequence: 0,
            points: 0,
            monitor: Vec::new(),
            monitor_units: 0,
            limit,
        })
    }
    pub(super) fn profile(&self) -> &MpeProfile {
        self.decoder.profile()
    }
    pub(super) fn active_count(&self) -> usize {
        self.decoder.active_count()
    }
    pub(super) fn last_input(&self) -> Option<Beat> {
        self.decoder.last_input()
    }
    pub(super) fn check_start(&self, at: Beat) -> Result<(), RecordingError> {
        if self.decoder.active_count() != 0 {
            return Err(RecordingError::State(
                "release all MPE voices and the sustain pedal before recording",
            ));
        }
        if self.decoder.last_input().is_some_and(|last| at < last) {
            return Err(RecordingError::Backwards);
        }
        Ok(())
    }
    pub(super) fn take_monitor_events(&mut self) -> Vec<MpeEvent> {
        self.monitor_units = 0;
        std::mem::take(&mut self.monitor)
    }
    pub(super) fn stage_input(
        &self,
        input: &CaptureInput,
        draft: Option<&TakeDraft>,
        next_identity: u64,
    ) -> Result<Stage, RecordingError> {
        let mut decoder = self.decoder.clone();
        let events = decoder.input(
            input.at,
            input.source_channel,
            input.message,
            input.target.clone(),
        )?;
        self.stage(decoder, events, draft, next_identity)
    }
    pub(super) fn stage_flush(
        &self,
        at: Beat,
        draft: Option<&TakeDraft>,
        next_identity: u64,
    ) -> Result<Stage, RecordingError> {
        let mut decoder = self.decoder.clone();
        let events = decoder.flush(at)?;
        self.stage(decoder, events, draft, next_identity)
    }
    fn stage(
        &self,
        decoder: MpeDecoder,
        events: Vec<MpeEvent>,
        draft: Option<&TakeDraft>,
        next_identity: u64,
    ) -> Result<Stage, RecordingError> {
        let added_units = events.iter().try_fold(0_usize, |sum, event| {
            sum.checked_add(event_units(event))
                .ok_or(RecordingError::Capacity)
        })?;
        let monitor_units = self
            .monitor_units
            .checked_add(added_units)
            .ok_or(RecordingError::Capacity)?;
        let reserved_cleanup = decoder
            .active_count()
            .checked_mul(3)
            .ok_or(RecordingError::Capacity)?;
        if monitor_units
            .checked_add(reserved_cleanup)
            .is_none_or(|count| count > self.limit)
        {
            return Err(RecordingError::Capacity);
        }
        let mut stage = Stage {
            decoder,
            changes: BTreeMap::new(),
            completed: Vec::new(),
            next_identity,
            next_sequence: self.next_sequence,
            points: self.points,
            monitor: Vec::new(),
            monitor_units,
        };
        if let Some(draft) = draft {
            for event in &events {
                stage.record_event(event, &self.pending, draft)?;
            }
            if stage
                .points
                .checked_add(stage.decoder.active_count())
                .is_none_or(|count| count > self.limit)
            {
                return Err(RecordingError::Capacity);
            }
        }
        stage.monitor = events;
        Ok(stage)
    }
    pub(super) fn commit_stage(&mut self, stage: Stage) -> (Vec<Event>, u64) {
        self.decoder = stage.decoder;
        for (voice, change) in stage.changes {
            self.commit_change(voice, change);
        }
        self.next_sequence = stage.next_sequence;
        self.points = stage.points;
        self.monitor.extend(stage.monitor);
        self.monitor_units = stage.monitor_units;
        (stage.completed, stage.next_identity)
    }
    fn commit_change(&mut self, voice: u64, change: Change) {
        if change.ended {
            self.pending.remove(&voice);
            return;
        }
        if let Some(builder) = change.new {
            self.pending.insert(voice, builder);
        }
        let builder = self.pending.get_mut(&voice).expect("staged voice exists");
        if let Some(release) = change.release {
            builder.release = Some(release);
        }
        builder.member.extend(change.member);
        builder.master.extend(change.master);
    }
    pub(super) fn checkpoint(&self) -> MpeCaptureCheckpoint {
        MpeCaptureCheckpoint {
            decoder: self.decoder.checkpoint(),
            pending: self.pending.values().cloned().collect(),
            next_sequence: self.next_sequence,
            points: self.points,
        }
    }
    pub(super) fn restore_validated(
        checkpoint: MpeCaptureCheckpoint,
    ) -> Result<Self, RecordingError> {
        let decoder = MpeDecoder::recover(checkpoint.decoder)?;
        let limit = point_limit(decoder.capacity())?;
        Ok(Self {
            decoder,
            pending: checkpoint
                .pending
                .into_iter()
                .map(|builder| (builder.voice, builder))
                .collect(),
            next_sequence: checkpoint.next_sequence,
            points: checkpoint.points,
            monitor: Vec::new(),
            monitor_units: 0,
            limit,
        })
    }
}

impl Stage {
    fn record_event(
        &mut self,
        event: &MpeEvent,
        pending: &BTreeMap<u64, Builder>,
        draft: &TakeDraft,
    ) -> Result<(), RecordingError> {
        match event {
            MpeEvent::Attack {
                id,
                at,
                target,
                pitch,
                velocity,
                provenance,
                member,
                master,
            } => {
                if *at < draft.started_at
                    || pending.contains_key(id)
                    || self.changes.contains_key(id)
                {
                    return Err(RecordingError::State(
                        "MPE attack does not belong to this take",
                    ));
                }
                let event_id = EventId::new(allocate_take_identity(&mut self.next_identity)?)?;
                let member = self.snapshot_points(member)?;
                let master = self.snapshot_points(master)?;
                self.changes.insert(
                    *id,
                    Change {
                        new: Some(Builder {
                            voice: *id,
                            id: event_id,
                            at: *at,
                            target: target.clone(),
                            pitch: *pitch,
                            velocity: *velocity,
                            provenance: provenance.clone(),
                            release: None,
                            member,
                            master,
                        }),
                        ..Change::default()
                    },
                );
            }
            MpeEvent::Release { id, at, velocity } => {
                let builder = self.builder(*id, pending)?;
                if at.checked_sub(builder.at)?.is_zero()
                    || builder.release.is_some()
                    || self
                        .changes
                        .get(id)
                        .is_some_and(|change| change.release.is_some())
                {
                    return Err(RecordingError::State(
                        "MPE release requires one positive physical note duration",
                    ));
                }
                self.changes.entry(*id).or_default().release = Some(PhysicalRelease {
                    at: *at,
                    velocity: *velocity,
                });
            }
            MpeEvent::Expression {
                id,
                at,
                master,
                control,
            } => {
                let offset = at.checked_sub(self.builder(*id, pending)?.at)?;
                let point = self.point(offset, *control)?;
                let change = self.changes.entry(*id).or_default();
                if *master {
                    change.master.push(point);
                } else {
                    change.member.push(point);
                }
            }
            MpeEvent::End { id, at } => {
                let builder = self.builder(*id, pending)?;
                let event = complete(builder, self.changes.get(id), *at, draft)?;
                self.completed.push(event);
                self.changes.entry(*id).or_default().ended = true;
            }
        }
        Ok(())
    }
    fn builder<'a>(
        &'a self,
        id: u64,
        pending: &'a BTreeMap<u64, Builder>,
    ) -> Result<&'a Builder, RecordingError> {
        self.changes
            .get(&id)
            .and_then(|change| change.new.as_ref())
            .or_else(|| pending.get(&id))
            .ok_or(RecordingError::State("MPE voice has no captured attack"))
    }
    fn snapshot_points(&mut self, controls: &[Control]) -> Result<Vec<Expression>, RecordingError> {
        controls
            .iter()
            .copied()
            .map(|control| self.point(Beat::ZERO, control))
            .collect()
    }
    fn point(&mut self, offset: Beat, control: Control) -> Result<Expression, RecordingError> {
        let sequence = self.next_sequence;
        self.next_sequence = sequence
            .checked_add(1)
            .filter(|next| *next <= MAX_EXPRESSION_SEQUENCE)
            .ok_or(RecordingError::Capacity)?;
        self.points = self.points.checked_add(1).ok_or(RecordingError::Capacity)?;
        Ok(Expression {
            offset,
            control,
            sequence: Some(sequence),
        })
    }
}
fn complete(
    builder: &Builder,
    change: Option<&Change>,
    at: Beat,
    draft: &TakeDraft,
) -> Result<Event, RecordingError> {
    let release = change
        .and_then(|change| change.release.as_ref())
        .or(builder.release.as_ref())
        .ok_or(RecordingError::State(
            "MPE voice ended before physical release",
        ))?;
    let mut note = Note::new(
        builder.pitch,
        builder.velocity,
        release.at.checked_sub(builder.at)?,
    )?;
    note.release_velocity = release.velocity;
    note.mpe = Some(builder.provenance.clone());
    note.expression_duration = Some(at.checked_sub(builder.at)?);
    note.expression = builder.member.clone();
    note.master_expression = builder.master.clone();
    if let Some(change) = change {
        note.expression.extend(change.member.iter().cloned());
        note.master_expression.extend(change.master.iter().cloned());
    }
    note.validate()?;
    Ok(Event::note(
        builder.id,
        within_loop(builder.at, draft.started_at, draft.length)?,
        builder.target.clone(),
        note,
    ))
}
fn point_limit(capacity: usize) -> Result<usize, RecordingError> {
    capacity
        .checked_mul(POINTS_PER_PACKET)
        .filter(|limit| (*limit as u128) <= u128::from(MAX_EXPRESSION_SEQUENCE))
        .ok_or(RecordingError::Capacity)
}
fn event_units(event: &MpeEvent) -> usize {
    match event {
        MpeEvent::Attack { member, master, .. } => 1 + member.len() + master.len(),
        _ => 1,
    }
}

impl MpeCaptureCheckpoint {
    pub(super) fn validate(
        &self,
        owner: &RecordingCheckpoint,
        setup: &CheckpointSetup,
        first_identity: u64,
    ) -> Result<(), RecordingError> {
        let decoder = MpeDecoder::recover(self.decoder.clone())?;
        let limit = point_limit(owner.capacity)?;
        checkpoint_require(
            owner.held.is_empty() && decoder.capacity() == owner.capacity,
            "MPE capture has ordinary held state or different capacity",
        )?;
        checkpoint_require(
            self.points <= limit && self.next_sequence <= MAX_EXPRESSION_SEQUENCE,
            "MPE curve budget is invalid",
        )?;
        let Some(draft) = &owner.draft else {
            checkpoint_require(
                matches!(
                    owner.state,
                    CaptureState::Armed
                        | CaptureState::Waiting { .. }
                        | CaptureState::Interrupted { .. }
                ),
                "MPE state requires a draft",
            )?;
            return checkpoint_require(
                self.pending.is_empty()
                    && self.points == 0
                    && self.next_sequence == 0
                    && owner.last_input.is_none()
                    && owner.packets == 0
                    && owner.next_identity == first_identity,
                "uncaptured MPE setup contains note builders",
            );
        };
        validate_draft_header(owner, setup, draft, &decoder)?;
        let mut allocated = BTreeSet::new();
        let mut sequences = BTreeSet::new();
        self.validate_completed(
            draft,
            owner.last_input.expect("validated last input"),
            &decoder,
            &mut allocated,
            &mut sequences,
        )?;
        self.validate_pending(
            owner,
            setup,
            draft,
            &decoder,
            &mut allocated,
            &mut sequences,
        )?;
        checkpoint_require(
            allocated.len() <= owner.packets
                && owner.next_identity.checked_sub(first_identity)
                    == u64::try_from(allocated.len()).ok()
                && allocated
                    .iter()
                    .all(|id| (first_identity..owner.next_identity).contains(&id.get())),
            "MPE native identity range is inconsistent",
        )?;
        checkpoint_require(
            sequences.len() == self.points
                && self.next_sequence == self.points as u64
                && sequences.iter().copied().eq(0..self.next_sequence),
            "MPE expression ordering is incomplete or duplicated",
        )?;
        checkpoint_require(
            self.points
                .checked_add(decoder.active_count())
                .is_some_and(|count| count <= limit),
            "MPE cleanup exceeds reserved curve capacity",
        )?;
        let mut candidate = setup.song.clone();
        candidate
            .phrases
            .get_mut(&setup.phrase)
            .expect("validated setup phrase")
            .events
            .extend(draft.events.iter().cloned());
        candidate.validate()?;
        Ok(())
    }
    fn validate_completed(
        &self,
        draft: &TakeDraft,
        last: Beat,
        decoder: &MpeDecoder,
        allocated: &mut BTreeSet<EventId>,
        sequences: &mut BTreeSet<u64>,
    ) -> Result<(), RecordingError> {
        let elapsed = last.checked_sub(draft.started_at)?;
        for event in &draft.events {
            checkpoint_require(
                allocated.insert(event.id) && event.at < draft.length && event.at <= elapsed,
                "invalid MPE completed note identity or position",
            )?;
            checkpoint_require(
                event.controls.is_empty() && event.fx.iter().all(Option::is_none),
                "MPE capture contains authored FX or channel controls",
            )?;
            let note = event.note.as_ref().ok_or(RecordingError::Checkpoint(
                "MPE capture contains a control event",
            ))?;
            note.validate()?;
            checkpoint_require(
                note.mpe
                    .as_ref()
                    .is_some_and(|provenance| provenance.profile() == decoder.profile())
                    && event.at.checked_add(note.expression_duration())? <= elapsed,
                "MPE completed note profile or lifetime differs from capture",
            )?;
            validate_curve(
                &note.expression,
                note.expression_duration(),
                false,
                sequences,
            )?;
            validate_curve(
                &note.master_expression,
                note.expression_duration(),
                true,
                sequences,
            )?;
        }
        Ok(())
    }
    fn validate_pending(
        &self,
        owner: &RecordingCheckpoint,
        setup: &CheckpointSetup,
        draft: &TakeDraft,
        decoder: &MpeDecoder,
        allocated: &mut BTreeSet<EventId>,
        sequences: &mut BTreeSet<u64>,
    ) -> Result<(), RecordingError> {
        let voices: BTreeMap<_, _> = decoder.voices().map(|voice| (voice.id(), voice)).collect();
        checkpoint_require(
            voices.len() == self.pending.len(),
            "MPE pending builders differ from decoder voices",
        )?;
        let mut seen = BTreeSet::new();
        for builder in &self.pending {
            let voice = voices
                .get(&builder.voice)
                .ok_or(RecordingError::Checkpoint(
                    "MPE builder has no decoder voice",
                ))?;
            checkpoint_require(
                seen.insert(builder.voice) && allocated.insert(builder.id),
                "duplicate MPE pending identity",
            )?;
            checkpoint_require(
                builder.at >= draft.started_at
                    && builder.at == voice.attack_at()
                    && builder.pitch == voice.pitch()
                    && builder.provenance.member() == voice.member()
                    && builder.provenance.profile() == decoder.profile()
                    && builder.release.as_ref().map(|release| release.at) == voice.released_at()
                    && (1..=127).contains(&builder.velocity),
                "MPE builder differs from decoder ownership",
            )?;
            if let Some(release) = &builder.release {
                checkpoint_require(
                    release.at > builder.at && release.velocity <= 127,
                    "invalid MPE physical release",
                )?;
            }
            setup.song.validate_target(&builder.target)?;
            let lifetime = owner
                .last_input
                .expect("validated last input")
                .checked_sub(builder.at)?;
            validate_curve(&builder.member, lifetime, false, sequences)?;
            validate_curve(&builder.master, lifetime, true, sequences)?;
        }
        checkpoint_require(
            !matches!(owner.state, CaptureState::Idle) || self.pending.is_empty(),
            "finished MPE capture has pending voices",
        )
    }
}
fn validate_draft_header(
    owner: &RecordingCheckpoint,
    setup: &CheckpointSetup,
    draft: &TakeDraft,
    decoder: &MpeDecoder,
) -> Result<(), RecordingError> {
    checkpoint_require(
        matches!(
            owner.state,
            CaptureState::Idle | CaptureState::Recording { .. } | CaptureState::Interrupted { .. }
        ),
        "uncaptured MPE state has a draft",
    )?;
    if let CaptureState::Recording { started_at } = owner.state {
        checkpoint_require(
            started_at == draft.started_at,
            "MPE recording start differs from draft",
        )?;
    }
    checkpoint_require(
        draft.phrase == setup.phrase
            && draft.occurrence == setup.occurrence
            && draft.expected_revision == setup.revision
            && draft.length == setup.length
            && draft.diagnostics.is_empty(),
        "MPE draft differs from its armed baseline",
    )?;
    let last = owner
        .last_input
        .ok_or(RecordingError::Checkpoint("MPE take has no timestamp"))?;
    checkpoint_require(
        last >= draft.started_at
            && decoder.last_input().is_none_or(|at| at <= last)
            && owner.packets <= decoder.packets(),
        "MPE timestamps or packet counts disagree",
    )
}
fn validate_curve(
    curve: &[Expression],
    lifetime: Beat,
    master: bool,
    sequences: &mut BTreeSet<u64>,
) -> Result<(), RecordingError> {
    let snapshot_length = validate_initial_snapshot(curve, master)?;
    let mut previous = None;
    for point in curve {
        point.control.validate()?;
        let sequence = point.sequence.ok_or(RecordingError::Checkpoint(
            "MPE expression lacks source ordering",
        ))?;
        checkpoint_require(
            point.offset <= lifetime
                && sequence <= MAX_EXPRESSION_SEQUENCE
                && sequences.insert(sequence)
                && previous.is_none_or(|(at, order)| point.offset >= at && sequence > order),
            "MPE curve time or source ordering is invalid",
        )?;
        previous = Some((point.offset, sequence));
    }
    let mut remaining = &curve[snapshot_length..];
    while let Some(point) = remaining.first() {
        let consumed = match point.control {
            Control::Bend { .. } | Control::Pressure { .. } => 1,
            Control::Cc { number, .. } if number == if master { 64 } else { 74 } => 1,
            Control::Cc {
                number: 101,
                value: 0,
            } => {
                validate_range_transaction(remaining)?;
                6
            }
            _ => {
                return Err(RecordingError::Checkpoint(
                    "unsupported control in MPE curve",
                ))
            }
        };
        remaining = &remaining[consumed..];
    }
    Ok(())
}
fn validate_range_transaction(curve: &[Expression]) -> Result<(), RecordingError> {
    let valid = curve.len() >= 6
        && curve[0].control
            == Control::Cc {
                number: 101,
                value: 0,
            }
        && curve[1].control
            == Control::Cc {
                number: 100,
                value: 0,
            }
        && matches!(
            curve[2].control,
            Control::Cc {
                number: 6,
                value: 0..=127
            }
        )
        && matches!(
            curve[3].control,
            Control::Cc {
                number: 38,
                value: 0..=99
            }
        )
        && curve[4].control
            == Control::Cc {
                number: 101,
                value: 127,
            }
        && curve[5].control
            == Control::Cc {
                number: 100,
                value: 127,
            }
        && curve[..6]
            .iter()
            .all(|point| point.offset == curve[0].offset)
        && consecutive_sequences(&curve[..6]);
    checkpoint_require(valid, "MPE curve contains an incomplete RPN transaction")
}
fn consecutive_sequences(curve: &[Expression]) -> bool {
    curve.windows(2).all(|pair| {
        pair[0]
            .sequence
            .and_then(|sequence| sequence.checked_add(1))
            == pair[1].sequence
    })
}
fn validate_initial_snapshot(curve: &[Expression], master: bool) -> Result<usize, RecordingError> {
    validate_range_transaction(curve)?;
    checkpoint_require(
        curve.len() >= 8 && matches!(curve[6].control, Control::Bend { .. }),
        "MPE curve lacks a verified controller snapshot",
    )?;
    let extra = if matches!(curve[7].control, Control::Pressure { .. }) {
        8
    } else {
        7
    };
    checkpoint_require(
        curve.get(extra).is_some_and(
            |point| matches!(point.control,Control::Cc{number,..} if number==if master{64}else{74}),
        ) && curve
            .iter()
            .take(extra + 1)
            .all(|point| point.offset.is_zero())
            && consecutive_sequences(&curve[..extra + 1]),
        "MPE initial controller snapshot has invalid scope or offset",
    )?;
    Ok(extra + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mpe::{BendRange, MpeZone};
    use serde_json::{json, Value};

    fn profile() -> MpeProfile {
        MpeProfile::new(
            "midi:exact".into(),
            MpeZone::lower(3).unwrap(),
            BendRange::new(48, 0).unwrap(),
            BendRange::new(2, 0).unwrap(),
        )
        .unwrap()
    }
    fn setup() -> (Editor, Occurrence) {
        let mut editor = Editor::new(Song::default()).unwrap();
        editor
            .apply(
                0,
                Command::AddPhrase {
                    track: TrackId::new(1).unwrap(),
                    slot: 0,
                },
            )
            .unwrap();
        let usage = editor.song().tracks[0].slots[0].as_ref().unwrap();
        let phrase = &editor.song().chains[&usage.chain].phrases[0];
        let occurrence = Occurrence {
            track: editor.song().tracks[0].id,
            chain_use: usage.id,
            phrase_use: phrase.id,
        };
        (editor, occurrence)
    }
    fn packet(at: u64, channel: u8, message: CaptureMessage) -> CaptureInput {
        CaptureInput {
            at: Beat::integer(at),
            source_channel: Channel::new(channel).unwrap(),
            target: Target::Channel {
                channel: Channel::new(channel).unwrap(),
                endpoint: None,
            },
            message,
        }
    }
    fn cc(number: u8, value: u8) -> CaptureMessage {
        CaptureMessage::Control(Control::Cc { number, value })
    }
    fn attack(recorder: &mut Recorder, at: u64, member: u8, pitch: u8) -> bool {
        recorder
            .input(packet(
                at,
                member,
                CaptureMessage::NoteOn {
                    pitch,
                    velocity: 100,
                },
            ))
            .unwrap()
    }
    fn release(recorder: &mut Recorder, at: u64, member: u8, pitch: u8, velocity: u8) -> bool {
        recorder
            .input(packet(
                at,
                member,
                CaptureMessage::NoteOff { pitch, velocity },
            ))
            .unwrap()
    }
    fn running(editor: &Editor, occurrence: Occurrence) -> Recorder {
        let mut recorder = Recorder::default();
        recorder.arm_mpe(editor, occurrence, profile()).unwrap();
        recorder.request_record(Beat::integer(4)).unwrap();
        recorder.advance(Beat::integer(4));
        recorder
    }
    fn snapshot(recorder: &Recorder) -> Value {
        serde_json::to_value(recorder.checkpoint()).unwrap()
    }
    fn recover(value: Value, editor: &Editor) -> Result<Recorder, String> {
        let checkpoint = serde_json::from_value(value).map_err(|error| format!("{error}"))?;
        Recorder::recover(checkpoint, editor).map_err(|error| error.to_string())
    }

    #[test]
    fn armed_input_primes_raw_monitor_snapshots_without_creating_take_data() {
        let (editor, path) = setup();
        let mut recorder = Recorder::default();
        recorder.arm_mpe(&editor, path, profile()).unwrap();
        assert_eq!(recorder.mpe_profile(), Some(&profile()));
        assert!(!recorder
            .input(packet(
                0,
                2,
                CaptureMessage::Control(Control::Bend { value: 8231 })
            ))
            .unwrap());
        assert!(!attack(&mut recorder, 1, 2, 60));
        assert_eq!(recorder.held_count(), 1);
        assert!(recorder.draft().is_none());
        assert!(recorder.request_record(Beat::integer(4)).is_err());
        assert_eq!(recorder.state(), &CaptureState::Armed);
        let monitor = recorder.take_mpe_monitor_events();
        let [MpeEvent::Attack { member, .. }] = monitor.as_slice() else {
            panic!("monitor attack expected")
        };
        assert!(member.contains(&Control::Bend { value: 8231 }));
        assert!(!member
            .iter()
            .any(|control| matches!(control, Control::Pressure { .. })));
        assert!(!release(&mut recorder, 2, 2, 60, 71));
        assert_eq!(recorder.held_count(), 0);
        recorder.take_mpe_monitor_events();
        recorder.request_record(Beat::integer(4)).unwrap();
        recorder.advance(Beat::integer(4));
        assert!(attack(&mut recorder, 4, 2, 62));
        release(&mut recorder, 5, 2, 62, 45);
        recorder.finish(Beat::integer(8)).unwrap();
        let draft = recorder.draft().unwrap();
        assert_eq!(draft.events.len(), 1);
        let note = draft.events[0].note.as_ref().unwrap();
        assert_eq!(note.pitch, 62);
        assert!(note
            .expression
            .iter()
            .any(|point| point.control == Control::Bend { value: 8231 }));
        assert_eq!(recorder.packets, 2);
        assert!(!snapshot(&recorder).to_string().contains("monitor"));
    }

    #[test]
    fn sustain_and_member_reuse_capture_physical_and_expression_lifetimes_separately() {
        let (mut editor, path) = setup();
        let original = editor.song().clone();
        let mut recorder = running(&editor, path);
        recorder.input(packet(4, 1, cc(64, 127))).unwrap();
        attack(&mut recorder, 4, 2, 60);
        release(&mut recorder, 5, 2, 60, 43);
        attack(&mut recorder, 6, 2, 60);
        recorder
            .input(packet(
                7,
                2,
                CaptureMessage::Control(Control::Bend { value: 8193 }),
            ))
            .unwrap();
        recorder
            .input(packet(
                7,
                1,
                CaptureMessage::Control(Control::Pressure { value: 83 }),
            ))
            .unwrap();
        recorder.input(packet(8, 1, cc(64, 0))).unwrap();
        release(&mut recorder, 9, 2, 60, 65);
        recorder.finish(Beat::integer(12)).unwrap();
        let events = &recorder.draft().unwrap().events;
        assert_eq!(events.len(), 2);
        let first = events[0].note.as_ref().unwrap();
        let second = events[1].note.as_ref().unwrap();
        assert_eq!(
            (
                first.duration,
                first.expression_duration(),
                first.release_velocity
            ),
            (Beat::ONE, Beat::integer(4), 43)
        );
        assert_eq!(
            (
                second.duration,
                second.expression_duration(),
                second.release_velocity
            ),
            (Beat::integer(3), Beat::integer(3), 65)
        );
        assert!(!first
            .expression
            .iter()
            .any(|point| point.control == Control::Bend { value: 8193 }));
        assert!(second
            .expression
            .iter()
            .any(|point| point.control == Control::Bend { value: 8193 }));
        assert!(first
            .master_expression
            .iter()
            .any(|point| point.offset == Beat::integer(3)
                && point.control == Control::Pressure { value: 83 }));
        assert!(second
            .master_expression
            .iter()
            .any(|point| point.offset == Beat::ONE
                && point.control == Control::Pressure { value: 83 }));
        assert!(first
            .expression
            .iter()
            .chain(&first.master_expression)
            .all(|point| point.sequence.is_some()));
        let recovered = recover(snapshot(&recorder), &editor).unwrap();
        assert_eq!(recovered.draft().unwrap().events, *events);
        recorder.commit(&mut editor).unwrap();
        editor.undo().unwrap();
        assert_eq!(editor.song(), &original);
    }

    #[test]
    fn fallible_mpe_packets_preserve_decoder_curves_ids_and_monitor_queue() {
        let (editor, path) = setup();
        let mut recorder = running(&editor, path);
        attack(&mut recorder, 4, 2, 60);
        let before = snapshot(&recorder);
        let queued = recorder.mpe.as_ref().unwrap().monitor.clone();
        for input in [
            packet(
                4,
                2,
                CaptureMessage::NoteOff {
                    pitch: 60,
                    velocity: 61,
                },
            ),
            packet(5, 2, cc(64, 127)),
            packet(
                5,
                2,
                CaptureMessage::NoteOn {
                    pitch: 61,
                    velocity: 100,
                },
            ),
        ] {
            assert!(recorder.input(input).is_err());
            assert_eq!(snapshot(&recorder), before);
            assert_eq!(recorder.mpe.as_ref().unwrap().monitor, queued);
        }
        assert!(recorder.finish_at(Beat::integer(4)).is_err());
        assert_eq!(snapshot(&recorder), before);
        release(&mut recorder, 5, 2, 60, 61);
        assert_eq!(
            recorder.draft().unwrap().events[0]
                .note
                .as_ref()
                .unwrap()
                .release_velocity,
            61
        );
    }

    #[test]
    fn boundary_rechecks_held_pre_roll_voices_and_emits_monitor_cleanup() {
        let (editor, path) = setup();
        let mut recorder = Recorder::default();
        recorder.arm_mpe(&editor, path, profile()).unwrap();
        recorder.request_record(Beat::integer(4)).unwrap();
        assert!(!attack(&mut recorder, 3, 2, 60));
        recorder.advance(Beat::integer(4));
        assert!(matches!(recorder.state(), CaptureState::Interrupted { .. }));
        assert!(recorder.draft().is_none());
        assert_eq!(recorder.held_count(), 0);
        let monitor = recorder.take_mpe_monitor_events();
        assert!(matches!(monitor.last(), Some(MpeEvent::End { .. })));
        assert!(recover(snapshot(&recorder), &editor).is_ok());
    }

    #[test]
    fn explicit_preroll_at_waiting_boundary_never_starts_take_and_cleans_held_monitor() {
        let (editor, path) = setup();
        let mut recorder = Recorder::default();
        recorder.arm_mpe(&editor, path, profile()).unwrap();
        recorder.request_record(Beat::ZERO).unwrap();
        recorder
            .prime_mpe(packet(
                0,
                2,
                CaptureMessage::NoteOn {
                    pitch: 60,
                    velocity: 99,
                },
            ))
            .unwrap();
        assert_eq!(
            recorder.state(),
            &CaptureState::Waiting {
                boundary: Beat::ZERO
            }
        );
        assert!(recorder.draft().is_none());
        assert_eq!(recorder.held_count(), 1);
        assert!(matches!(
            recorder.take_mpe_monitor_events().as_slice(),
            [MpeEvent::Attack { .. }]
        ));
        recorder.advance(Beat::ZERO);
        assert!(matches!(recorder.state(), CaptureState::Interrupted { .. }));
        assert!(recorder.draft().is_none());
        assert_eq!(recorder.held_count(), 0);
        assert!(matches!(
            recorder.take_mpe_monitor_events().as_slice(),
            [
                MpeEvent::Release { .. },
                MpeEvent::Expression { master: true, .. },
                MpeEvent::End { .. }
            ]
        ));
        assert!(recover(snapshot(&recorder), &editor).is_ok());
    }

    #[test]
    fn explicit_preroll_preserves_cached_controls_and_rejects_other_capture_states() {
        let (editor, path) = setup();
        let input = packet(0, 2, CaptureMessage::Control(Control::Bend { value: 8337 }));
        let mut recorder = Recorder::default();
        assert!(recorder.prime_mpe(input.clone()).is_err());
        recorder.arm(&editor, path).unwrap();
        let ordinary = snapshot(&recorder);
        assert!(recorder.prime_mpe(input.clone()).is_err());
        assert_eq!(snapshot(&recorder), ordinary);
        recorder.discard();
        recorder.arm_mpe(&editor, path, profile()).unwrap();
        recorder.request_record(Beat::ZERO).unwrap();
        recorder.prime_mpe(input.clone()).unwrap();
        assert!(recorder.draft().is_none());
        recorder.advance(Beat::ZERO);
        let recording = snapshot(&recorder);
        assert!(recorder.prime_mpe(input).is_err());
        assert_eq!(snapshot(&recorder), recording);
        attack(&mut recorder, 1, 2, 60);
        let monitor = recorder.take_mpe_monitor_events();
        assert!(
            matches!(monitor.as_slice(), [MpeEvent::Attack { member, .. }]
            if member.contains(&Control::Bend { value: 8337 }))
        );
        release(&mut recorder, 2, 2, 60, 71);
        assert!(recorder.draft().unwrap().events[0]
            .note
            .as_ref()
            .unwrap()
            .expression
            .iter()
            .any(|point| point.control == Control::Bend { value: 8337 }));
    }

    #[test]
    fn interrupted_zero_duration_attack_remains_recoverable_and_monitor_is_never_replayed() {
        let (editor, path) = setup();
        let mut recorder = running(&editor, path);
        attack(&mut recorder, 4, 2, 60);
        assert!(recorder.interrupt(Beat::integer(4), "disconnect").is_err());
        assert_eq!(recorder.held_count(), 1);
        let mut recovered = recover(snapshot(&recorder), &editor).unwrap();
        assert!(recovered.take_mpe_monitor_events().is_empty());
        assert!(matches!(
            recovered.state(),
            CaptureState::Interrupted { .. }
        ));
        recovered.finish_at(Beat::integer(5)).unwrap();
        assert_eq!(recovered.held_count(), 0);
        let note = recovered.draft().unwrap().events[0].note.as_ref().unwrap();
        assert_eq!(note.duration, Beat::ONE);
        assert_eq!(note.expression_duration(), Beat::ONE);
        assert!(note
            .master_expression
            .iter()
            .any(|point| point.offset == Beat::ONE
                && point.control
                    == Control::Cc {
                        number: 64,
                        value: 0
                    }));
        assert!(recovered
            .take_mpe_monitor_events()
            .iter()
            .all(|event| !matches!(event, MpeEvent::Attack { .. })));
    }

    #[test]
    fn packet_exhaustion_interrupts_and_closes_known_mpe_voices_without_accepting_new_input() {
        let (editor, path) = setup();
        let mut recorder = Recorder::with_capacity(3).unwrap();
        recorder.arm_mpe(&editor, path, profile()).unwrap();
        recorder.request_record(Beat::ZERO).unwrap();
        attack(&mut recorder, 0, 2, 60);
        recorder
            .input(packet(
                1,
                2,
                CaptureMessage::Control(Control::Bend { value: 8193 }),
            ))
            .unwrap();
        recorder
            .input(packet(
                2,
                2,
                CaptureMessage::Control(Control::Bend { value: 8194 }),
            ))
            .unwrap();
        assert!(recorder
            .input(packet(
                3,
                2,
                CaptureMessage::Control(Control::Bend { value: 8195 })
            ))
            .is_err());
        assert!(matches!(recorder.state(), CaptureState::Interrupted { .. }));
        assert_eq!(recorder.held_count(), 0);
        assert_eq!(recorder.packets, 3);
        let note = recorder.draft().unwrap().events[0].note.as_ref().unwrap();
        assert_eq!(note.duration, Beat::integer(2));
        assert!(!note
            .expression
            .iter()
            .any(|point| point.control == Control::Bend { value: 8195 }));
        assert!(recover(snapshot(&recorder), &editor).is_ok());
    }

    fn active_checkpoint() -> (Editor, Recorder) {
        let (editor, path) = setup();
        let mut recorder = running(&editor, path);
        recorder.input(packet(4, 1, cc(64, 127))).unwrap();
        attack(&mut recorder, 4, 2, 60);
        release(&mut recorder, 5, 2, 60, 43);
        attack(&mut recorder, 6, 2, 62);
        (editor, recorder)
    }
    #[test]
    fn recovery_proves_decoder_builder_identity_curves_ranges_and_baseline() {
        let (editor, recorder) = active_checkpoint();
        let base = snapshot(&recorder);
        assert!(recover(base.clone(), &editor).is_ok());
        let changes: Vec<fn(&mut Value)> = vec![
            |v| v["mpe"]["pending"][0]["voice"] = json!(999),
            |v| v["mpe"]["pending"][0]["pitch"] = json!(61),
            |v| v["mpe"]["pending"][0]["release"]["at"] = json!(Beat::integer(6)),
            |v| v["mpe"]["pending"][0]["release"] = Value::Null,
            |v| v["mpe"]["pending"][0]["id"] = v["mpe"]["pending"][1]["id"].clone(),
            |v| v["mpe"]["pending"][0]["member"][0]["sequence"] = json!(9999),
            |v| v["mpe"]["pending"][0]["master"][0]["sequence"] = json!(0),
            |v| v["mpe"]["pending"][0]["member"][3]["control"]["value"] = json!(100),
            |v| v["mpe"]["pending"][0]["provenance"]["profile"]["source"] = json!("other"),
            |v| {
                v["mpe"]["pending"][0]["target"] = json!({"kind":"instrument","id":999,"channel":2})
            },
            |v| {
                v["mpe"]["pending"]
                    .as_array_mut()
                    .unwrap()
                    .pop()
                    .map(|_| ())
                    .unwrap()
            },
            |v| v["mpe"]["points"] = json!(0),
            |v| v["mpe"]["next_sequence"] = json!(0),
            |v| v["packets"] = json!(0),
            |v| v["next_identity"] = json!(1),
        ];
        for (index, change) in changes.into_iter().enumerate() {
            let mut value = base.clone();
            change(&mut value);
            assert!(recover(value, &editor).is_err(), "mutation {index}");
        }
        let mut changed = Editor::new(editor.song().clone()).unwrap();
        changed
            .apply(
                0,
                Command::RenameSong {
                    name: "Other".into(),
                },
            )
            .unwrap();
        assert!(recover(base, &changed).is_err());
    }

    #[test]
    fn recovery_requires_complete_same_timestamp_rpn_updates() {
        let (editor, path) = setup();
        let mut recorder = running(&editor, path);
        attack(&mut recorder, 4, 2, 60);
        for channel in [1, 2] {
            recorder.input(packet(5, channel, cc(101, 0))).unwrap();
            recorder.input(packet(5, channel, cc(100, 0))).unwrap();
            recorder.input(packet(5, channel, cc(6, 12))).unwrap();
        }
        let base = snapshot(&recorder);
        assert!(recover(base.clone(), &editor).is_ok());
        for scope in ["member", "master"] {
            for (index, control) in [
                (8, json!({"kind":"bend","value":8192})),
                (9, json!({"kind":"cc","number":100,"value":6})),
                (11, json!({"kind":"cc","number":38,"value":100})),
                (12, json!({"kind":"cc","number":101,"value":0})),
            ] {
                let mut malformed = base.clone();
                malformed["mpe"]["pending"][0][scope][index]["control"] = control;
                assert!(
                    recover(malformed, &editor).is_err(),
                    "{scope} point {index}"
                );
            }
            let mut malformed = base.clone();
            malformed["mpe"]["pending"][0][scope][8]["offset"] = json!(Beat::ZERO);
            assert!(recover(malformed, &editor).is_err());
        }
        release(&mut recorder, 6, 2, 60, 31);
        assert!(recover(snapshot(&recorder), &editor).is_ok());
    }

    #[test]
    fn ordinary_recovery_rejects_injected_mpe_lifetime_or_master_expression() {
        let (editor, path) = setup();
        let mut recorder = Recorder::default();
        recorder.arm(&editor, path).unwrap();
        recorder.request_record(Beat::integer(4)).unwrap();
        attack(&mut recorder, 4, 2, 60);
        release(&mut recorder, 5, 2, 60, 31);
        let base = snapshot(&recorder);
        assert!(recover(base.clone(), &editor).is_ok());
        let mut lifetime = base.clone();
        lifetime["draft"]["events"][0]["note"]["expression_duration"] = json!(Beat::ONE);
        assert!(recover(lifetime, &editor).is_err());
        let mut curve = base;
        curve["draft"]["events"][0]["note"]["master_expression"] = json!([{
            "offset": Beat::ZERO,
            "control": {"kind":"bend","value":8192}
        }]);
        assert!(recover(curve, &editor).is_err());
    }

    #[test]
    fn master_curve_fanout_is_bounded_before_packet_limit_with_cleanup_reserved() {
        let (editor, path) = setup();
        let mut recorder = Recorder::with_capacity(200).unwrap();
        recorder.arm_mpe(&editor, path, profile()).unwrap();
        recorder.request_record(Beat::ZERO).unwrap();
        recorder.input(packet(0, 1, cc(64, 127))).unwrap();
        for voice in 0..20 {
            attack(&mut recorder, voice * 2 + 1, 2, 60);
            release(&mut recorder, voice * 2 + 2, 2, 60, 31);
            recorder.take_mpe_monitor_events();
        }
        recorder.input(packet(41, 1, cc(101, 0))).unwrap();
        recorder.input(packet(41, 1, cc(100, 0))).unwrap();
        let mut exhausted = false;
        for value in 0..100 {
            if recorder
                .input(packet(42 + u64::from(value), 1, cc(6, value)))
                .is_err()
            {
                exhausted = true;
                break;
            }
            recorder.take_mpe_monitor_events();
        }
        assert!(exhausted);
        assert!(recorder.packets < 200);
        assert_eq!(recorder.held_count(), 0);
        assert!(matches!(recorder.state(), CaptureState::Interrupted { .. }));
        assert_eq!(recorder.draft().unwrap().events.len(), 20);
        assert!(recorder.mpe.as_ref().unwrap().points <= 200 * POINTS_PER_PACKET);
        assert!(recover(snapshot(&recorder), &editor).is_ok());
    }

    #[test]
    fn undrained_preroll_monitor_has_its_own_bound_and_retains_cleanup() {
        let (editor, path) = setup();
        let mut recorder = Recorder::with_capacity(200).unwrap();
        recorder.arm_mpe(&editor, path, profile()).unwrap();
        recorder.input(packet(0, 1, cc(64, 127))).unwrap();
        for voice in 0..20 {
            attack(&mut recorder, voice * 2 + 1, 2, 60);
            release(&mut recorder, voice * 2 + 2, 2, 60, 31);
        }
        recorder.input(packet(41, 1, cc(101, 0))).unwrap();
        recorder.input(packet(41, 1, cc(100, 0))).unwrap();
        let mut exhausted = false;
        for value in 0..100 {
            if recorder
                .input(packet(42 + u64::from(value), 1, cc(6, value)))
                .is_err()
            {
                exhausted = true;
                break;
            }
        }
        assert!(exhausted);
        assert!(recorder.draft().is_none());
        assert!(matches!(recorder.state(), CaptureState::Interrupted { .. }));
        assert_eq!(recorder.held_count(), 0);
        let capture = recorder.mpe.as_ref().unwrap();
        assert_eq!(capture.points, 0);
        assert!(capture.decoder.packets() < 200);
        assert!(capture.monitor_units <= capture.limit);
        assert!(matches!(
            recorder.take_mpe_monitor_events().last(),
            Some(MpeEvent::End { .. })
        ));
        assert_eq!(recorder.mpe.as_ref().unwrap().monitor_units, 0);
        assert!(recover(snapshot(&recorder), &editor).is_ok());
    }
}
