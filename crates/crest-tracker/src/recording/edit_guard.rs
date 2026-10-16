//! The retained take permits edits only to other existing phrase definitions.
use super::*;

#[derive(Debug, Clone)]
pub struct RecordingEditGuard {
    pub(super) baseline: Arc<Song>,
    pub(super) phrase: PhraseId,
    pub(super) allocated: u64,
}

impl RecordingEditGuard {
    /// Check the complete candidate, including Undo/Redo and compound commands,
    /// before an editor transaction changes content or history.
    pub(crate) fn validate(&self, candidate: &Song) -> Result<(), DocumentError> {
        let mut context = candidate.clone();
        context.phrases = self.baseline.phrases.clone();
        if context != *self.baseline
            || candidate.phrases.keys().ne(self.baseline.phrases.keys())
            || candidate.phrases.get(&self.phrase) != self.baseline.phrases.get(&self.phrase)
        {
            return Err(DocumentError::Invalid(
                "finish the retained take before changing its phrase, arrangement, routing or Song settings",
            ));
        }
        candidate
            .next_identity()?
            .checked_add(self.allocated)
            .ok_or(DocumentError::Invalid("capture identity range exhausted"))?;
        Ok(())
    }
}

/// A checked translation of the contiguous private capture identity range.
/// It never changes decoder voice IDs, packet ordering or musical time.
pub(super) struct CaptureIdentityShift {
    first: u64,
    next_first: u64,
    pub(super) next: u64,
}

impl CaptureIdentityShift {
    fn new(first: u64, next: u64, next_first: u64) -> Result<Self, DocumentError> {
        let count = next.checked_sub(first).ok_or(DocumentError::Invalid(
            "capture identity range is inconsistent",
        ))?;
        let next = next_first
            .checked_add(count)
            .ok_or(DocumentError::Invalid("capture identity range exhausted"))?;
        Ok(Self {
            first,
            next_first,
            next,
        })
    }
    pub(super) fn event(&self, id: EventId) -> EventId {
        EventId::new(self.next_first + (id.get() - self.first))
            .expect("allocated capture identity stays in its checked range")
    }
}

impl Recorder {
    /// A transaction guard for the current take, including space for all IDs
    /// already allocated to completed and held notes or controls.
    pub fn edit_guard(&self) -> Option<RecordingEditGuard> {
        self.setup.as_ref().map(|setup| RecordingEditGuard {
            baseline: setup.song.clone(),
            phrase: setup.phrase,
            allocated: self.next_identity - setup.first_identity,
        })
    }

    /// Adopt an independently edited phrase baseline without retiming the take.
    /// Validation and identity exhaustion leave the recorder unchanged. The host
    /// must use the same guard before publishing the editor transaction.
    pub fn rebase_other_phrase_edits(&mut self, editor: &Editor) -> Result<(), RecordingError> {
        let Some(setup) = &self.setup else {
            return Ok(());
        };
        if setup.revision == editor.revision() && *setup.song == *editor.song() {
            return Ok(());
        }
        self.edit_guard()
            .expect("setup checked")
            .validate(editor.song())?;
        let first = editor.song().next_identity()?;
        let shift = CaptureIdentityShift::new(setup.first_identity, self.next_identity, first)?;
        self.shift_capture_identities(&shift);
        let setup = self.setup.as_mut().expect("setup checked");
        setup.song = Arc::new(editor.song().clone());
        setup.revision = editor.revision();
        setup.first_identity = first;
        self.next_identity = shift.next;
        if let Some(draft) = &mut self.draft {
            draft.expected_revision = editor.revision();
        }
        Ok(())
    }

    fn shift_capture_identities(&mut self, shift: &CaptureIdentityShift) {
        if let Some(draft) = &mut self.draft {
            for event in &mut draft.events {
                event.id = shift.event(event.id);
            }
        }
        for held in self.held.values_mut().flatten() {
            match held {
                Held::Active(attack) => attack.id = shift.event(attack.id),
                Held::Retired(id) => *id = shift.event(*id),
            }
        }
        if let Some(mpe) = &mut self.mpe {
            mpe.shift_event_identities(shift);
        }
    }
}
