use crate::kernel::PatchId;
use crate::synth::{PreparedAssetFootprint, PreparedSampleVisualization};

/// Callback-ready, one-voice audition prepared entirely on worker ownership.
///
/// The port is intentionally capability-neutral. A preparer may support it
/// when its asset can be auditioned; unsupported capabilities refuse during
/// worker preparation. Implementations own all immutable media and fixed
/// voice state before publication. Every method is called only by the audio
/// callback and must remain bounded, allocation-free, lock-free, and
/// destructor-free.
pub trait PreparedAudition: Send {
    fn patch_id(&self) -> PatchId;
    fn start(&mut self);
    fn stop(&mut self);
    /// Adds audition audio to the supplied origin-Patch stem.
    fn render(&mut self, interleaved_stereo: &mut [f32], frame_count: usize);
    fn is_playing(&self) -> bool;
    /// Normalized finite file playhead, or zero while idle.
    fn playhead(&self) -> f32;
    /// Optional immutable-media footprint used by complete-graph admission.
    fn prepared_asset_footprint(&self) -> Option<&PreparedAssetFootprint> {
        None
    }
    /// Optional bounded waveform summary created on worker ownership.
    fn prepared_sample_visualization(&self) -> Option<&PreparedSampleVisualization> {
        None
    }
}
