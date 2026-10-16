use crate::kernel::patch_id::PatchId;
use crate::real_time::parameter_snapshot::{ParameterSnapshot, MAX_ACTIVE_PATCHES};
use core::fmt;

/// The reason a prepared Patch audio block could not be used.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PatchAudioBlockError {
    /// Audio storage requires at least one frame.
    InvalidMaxFrames,
    /// Interleaved stereo storage cannot be represented for the requested size.
    SampleCapacityExceeded,
    /// The control thread could not reserve the requested prepared storage.
    StorageAllocationFailed,
    /// A callback requested more frames than were prepared.
    FrameCapacityExceeded { requested: usize, capacity: usize },
    /// An inactive parameter entry appeared inside the active Patch prefix.
    InactivePatch { index: usize },
}

impl fmt::Display for PatchAudioBlockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::InvalidMaxFrames => {
                formatter.write_str("Patch audio storage requires at least one frame")
            }
            Self::SampleCapacityExceeded => {
                formatter.write_str("Patch audio frame count exceeds addressable stereo storage")
            }
            Self::StorageAllocationFailed => {
                formatter.write_str("Patch audio storage allocation failed")
            }
            Self::FrameCapacityExceeded {
                requested,
                capacity,
            } => write!(
                formatter,
                "Patch audio block requested {requested} frames; prepared capacity is {capacity}"
            ),
            Self::InactivePatch { index } => {
                write!(formatter, "Patch audio block parameter {index} is inactive")
            }
        }
    }
}

impl std::error::Error for PatchAudioBlockError {}

/// Prepared interleaved stereo storage for one instrument Patch slot.
///
/// The identity and active frame count are assigned by PatchAudioBlock.
/// Consumers can read only the active sample prefix, while synthesis receives
/// mutable access only after matching both the Patch index and identity.
#[derive(Debug)]
pub struct PatchStereoStem {
    patch_id: Option<PatchId>,
    frame_count: usize,
    samples: Vec<f32>,
}

impl PatchStereoStem {
    fn unallocated() -> Self {
        Self {
            patch_id: None,
            frame_count: 0,
            samples: Vec::new(),
        }
    }

    fn allocate(&mut self, sample_capacity: usize) -> Result<(), PatchAudioBlockError> {
        self.samples
            .try_reserve_exact(sample_capacity)
            .map_err(|_| PatchAudioBlockError::StorageAllocationFailed)?;
        self.samples.resize(sample_capacity, 0.0);
        Ok(())
    }

    fn begin(&mut self, patch_id: PatchId, frame_count: usize) {
        self.patch_id = Some(patch_id);
        self.frame_count = frame_count;
        self.samples[..frame_count * 2].fill(0.0);
    }

    fn deactivate(&mut self) {
        self.patch_id = None;
        self.frame_count = 0;
    }

    fn clear(&mut self) {
        let sample_count = self.frame_count * 2;
        self.samples[..sample_count].fill(0.0);
    }

    fn samples_mut(&mut self) -> &mut [f32] {
        let sample_count = self.frame_count * 2;
        &mut self.samples[..sample_count]
    }

    /// Returns the Patch identity assigned to this active stem.
    pub const fn patch_id(&self) -> Option<PatchId> {
        self.patch_id
    }

    /// Returns the number of active stereo frames.
    pub const fn frame_count(&self) -> usize {
        self.frame_count
    }

    /// Returns exactly the active interleaved stereo sample prefix.
    pub fn samples(&self) -> &[f32] {
        &self.samples[..self.frame_count * 2]
    }
}

/// Caller-owned synthesis storage preserving one stem per active Patch.
///
/// `prepare` is the sole allocation point and is called by
/// `PreparedGraphBuilder` on the control/worker side. After preparation,
/// beginning, clearing, filling, and reading a block do not allocate.
#[derive(Debug)]
pub struct PatchAudioBlock {
    frame_count: usize,
    patch_count: usize,
    stems: [PatchStereoStem; MAX_ACTIVE_PATCHES],
    max_frames: usize,
}

impl PatchAudioBlock {
    /// Allocates fixed storage for every Patch and the renderer's maximum frame
    /// count.
    ///
    /// This is a control-thread operation. The returned value must be retained
    /// and reused by the audio renderer rather than reconstructed in a callback.
    pub fn prepare(max_frames: usize) -> Result<Self, PatchAudioBlockError> {
        if max_frames == 0 {
            return Err(PatchAudioBlockError::InvalidMaxFrames);
        }
        let sample_capacity = max_frames
            .checked_mul(2)
            .ok_or(PatchAudioBlockError::SampleCapacityExceeded)?;

        let mut stems = std::array::from_fn(|_| PatchStereoStem::unallocated());
        for stem in &mut stems {
            stem.allocate(sample_capacity)?;
        }

        Ok(Self {
            frame_count: 0,
            patch_count: 0,
            stems,
            max_frames,
        })
    }

    /// Assigns the active ParameterSnapshot prefix to matching stem indexes and
    /// clears every active sample.
    ///
    /// No allocation, locking, blocking, I/O, logging, or destruction occurs.
    pub fn begin_render(
        &mut self,
        parameters: &ParameterSnapshot,
        frame_count: usize,
    ) -> Result<(), PatchAudioBlockError> {
        if frame_count > self.max_frames {
            return Err(PatchAudioBlockError::FrameCapacityExceeded {
                requested: frame_count,
                capacity: self.max_frames,
            });
        }

        for (index, patch) in parameters.patches().iter().enumerate() {
            if patch.patch_id().is_none() {
                return Err(PatchAudioBlockError::InactivePatch { index });
            }
        }

        self.frame_count = frame_count;
        self.patch_count = parameters.patch_count();

        for (index, patch) in parameters.patches().iter().enumerate() {
            if let Some(patch_id) = patch.patch_id() {
                self.stems[index].begin(patch_id, frame_count);
            }
        }
        for stem in &mut self.stems[self.patch_count..] {
            stem.deactivate();
        }

        Ok(())
    }

    /// Clears all active samples without changing Patch identities or sizes.
    pub fn clear(&mut self) {
        for stem in &mut self.stems[..self.patch_count] {
            stem.clear();
        }
    }

    /// Returns mutable samples only when both the ParameterSnapshot index and
    /// Patch identity match the active stem.
    pub fn stem_mut(&mut self, index: usize, patch_id: PatchId) -> Option<&mut [f32]> {
        if index >= self.patch_count {
            return None;
        }
        let stem = &mut self.stems[index];
        if stem.patch_id() != Some(patch_id) {
            return None;
        }
        Some(stem.samples_mut())
    }

    /// Returns one active stem only when its index and identity both match.
    pub fn stem(&self, index: usize, patch_id: PatchId) -> Option<&PatchStereoStem> {
        let stem = self.stems().get(index)?;
        (stem.patch_id() == Some(patch_id)).then_some(stem)
    }

    /// Returns all active stems in ParameterSnapshot order.
    pub fn stems(&self) -> &[PatchStereoStem] {
        &self.stems[..self.patch_count]
    }

    /// Returns the complete prepared fixed Patch storage.
    pub const fn storage(&self) -> &[PatchStereoStem; MAX_ACTIVE_PATCHES] {
        &self.stems
    }

    /// Returns the active frame count shared by every active stem.
    pub const fn frame_count(&self) -> usize {
        self.frame_count
    }

    /// Returns the number of active Patch stems.
    pub const fn patch_count(&self) -> usize {
        self.patch_count
    }

    /// Returns the maximum frame count allocated during preparation.
    pub const fn max_frames(&self) -> usize {
        self.max_frames
    }
}

#[cfg(test)]
mod tests {
    use super::{PatchAudioBlock, PatchAudioBlockError};
    use crate::kernel::patch_id::PatchId;
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_state::MixerState;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::parameter_snapshot::{
        ParameterSnapshot, RtPatchParameters, MAX_ACTIVE_PATCHES,
    };

    fn patch(id: u32) -> RtPatchParameters {
        RtPatchParameters::new(PatchId::new(id).unwrap(), PatchOutput::default())
    }

    fn snapshot(ids: &[u32]) -> ParameterSnapshot {
        let mut patches = std::array::from_fn::<_, MAX_ACTIVE_PATCHES, _>(|_| patch(1));
        for (slot, id) in patches.iter_mut().zip(ids) {
            *slot = patch(*id);
        }
        let global = GlobalParameters::new(0.0).unwrap();
        ParameterSnapshot::new(1, global, MixerState::default(), &patches[..ids.len()]).unwrap()
    }

    #[test]
    fn prepares_fixed_stereo_capacity_for_every_patch() {
        let block = PatchAudioBlock::prepare(4).unwrap();

        assert_eq!(block.max_frames(), 4);
        assert_eq!(block.frame_count(), 0);
        assert_eq!(block.patch_count(), 0);
        assert_eq!(block.storage().len(), MAX_ACTIVE_PATCHES);
        assert!(block
            .storage()
            .iter()
            .all(|stem| stem.samples.capacity() >= 8));
    }

    #[test]
    fn active_stems_follow_snapshot_identity_and_index() {
        let parameters = snapshot(&[11, 22]);
        let mut block = PatchAudioBlock::prepare(3).unwrap();

        block.begin_render(&parameters, 2).unwrap();
        block
            .stem_mut(0, PatchId::new(11).unwrap())
            .unwrap()
            .copy_from_slice(&[1.0, 2.0, 3.0, 4.0]);
        block
            .stem_mut(1, PatchId::new(22).unwrap())
            .unwrap()
            .copy_from_slice(&[5.0, 6.0, 7.0, 8.0]);

        assert_eq!(block.frame_count(), 2);
        assert_eq!(block.patch_count(), 2);
        assert_eq!(
            block.stem(0, PatchId::new(11).unwrap()).unwrap().samples(),
            [1.0, 2.0, 3.0, 4.0]
        );
        assert_eq!(
            block.stem(1, PatchId::new(22).unwrap()).unwrap().samples(),
            [5.0, 6.0, 7.0, 8.0]
        );
        assert!(block.stem_mut(0, PatchId::new(22).unwrap()).is_none());
        assert!(block.stem(2, PatchId::new(11).unwrap()).is_none());
    }

    #[test]
    fn starting_the_next_block_rekeys_clears_and_deactivates_stems() {
        let mut block = PatchAudioBlock::prepare(3).unwrap();
        block.begin_render(&snapshot(&[1, 2]), 3).unwrap();
        block
            .stem_mut(0, PatchId::new(1).unwrap())
            .unwrap()
            .fill(0.75);
        block
            .stem_mut(1, PatchId::new(2).unwrap())
            .unwrap()
            .fill(0.5);

        block.begin_render(&snapshot(&[2]), 1).unwrap();

        assert_eq!(block.patch_count(), 1);
        assert_eq!(
            block.stem(0, PatchId::new(2).unwrap()).unwrap().samples(),
            [0.0, 0.0]
        );
        assert_eq!(block.storage()[1].patch_id(), None);
        assert_eq!(block.storage()[1].frame_count(), 0);
    }

    #[test]
    fn clearing_and_filling_reuse_prepared_storage() {
        let mut block = PatchAudioBlock::prepare(4).unwrap();
        let capacities: [usize; MAX_ACTIVE_PATCHES] =
            std::array::from_fn(|index| block.storage()[index].samples.capacity());
        block.begin_render(&snapshot(&[7, 8]), 4).unwrap();

        block
            .stem_mut(0, PatchId::new(7).unwrap())
            .unwrap()
            .fill(1.0);
        block
            .stem_mut(1, PatchId::new(8).unwrap())
            .unwrap()
            .fill(-1.0);
        block.clear();

        assert!(block
            .stems()
            .iter()
            .all(|stem| stem.samples().iter().all(|sample| *sample == 0.0)));
        assert_eq!(
            capacities,
            std::array::from_fn(|index| block.storage()[index].samples.capacity())
        );
    }

    #[test]
    fn rejects_invalid_or_excessive_frame_capacity_without_mutation() {
        assert_eq!(
            PatchAudioBlock::prepare(0).unwrap_err(),
            PatchAudioBlockError::InvalidMaxFrames
        );

        let mut block = PatchAudioBlock::prepare(2).unwrap();
        let error = block.begin_render(&snapshot(&[1]), 3).unwrap_err();

        assert_eq!(
            error,
            PatchAudioBlockError::FrameCapacityExceeded {
                requested: 3,
                capacity: 2
            }
        );
        assert_eq!(block.patch_count(), 0);
        assert_eq!(block.frame_count(), 0);
    }
}
