pub mod midi_channel;

pub use midi_channel::{MidiChannel, MidiChannelError};
pub mod midi_message;
pub mod patch_id;
pub use patch_id::{PatchId, PatchIdError};
pub mod patch_capacity;
pub use patch_capacity::MAX_ACTIVE_PATCHES;
