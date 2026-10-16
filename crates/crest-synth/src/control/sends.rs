use crate::mixer::bus_id::BusId;
use crate::synth::{EffectCapabilityId, EffectSlotId, ParameterId};
use serde::{Deserialize, Serialize};

/// Semantic intent for the Mixer-owned sends screen.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SendAction {
    Open,
    Rename {
        bus: BusId,
        name: String,
    },
    CancelRename,
    SetEffect {
        bus: BusId,
        slot_id: EffectSlotId,
        entry: Option<EffectCapabilityId>,
    },
}

/// Stable identity within one send chain. Slots survive edits and deletion.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SendControlId {
    Name {
        bus: BusId,
    },
    Level {
        bus: BusId,
    },
    EffectSlot {
        bus: BusId,
        slot_id: EffectSlotId,
    },
    EffectParameter {
        bus: BusId,
        slot_id: EffectSlotId,
        parameter: ParameterId,
    },
    Choice {
        bus: BusId,
        slot_id: EffectSlotId,
        entry: String,
    },
}
impl SendControlId {
    pub const fn bus(&self) -> BusId {
        match self {
            Self::Name { bus }
            | Self::Level { bus }
            | Self::EffectSlot { bus, .. }
            | Self::EffectParameter { bus, .. }
            | Self::Choice { bus, .. } => *bus,
        }
    }
}
