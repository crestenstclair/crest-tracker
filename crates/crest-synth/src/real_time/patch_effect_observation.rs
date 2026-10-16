use crate::kernel::PatchId;
use crate::real_time::MAX_ACTIVE_PATCHES;
use crate::synth::effect_slot_id::MAX_EFFECT_SLOTS;
use serde::Serialize;

/// Fixed callback-local measurement covering every ordered effect position:
/// one `PatchEffectObservation` per grid position rather than one per Patch.
///
/// Unoccupied positions stay `PatchEffectObservation::EMPTY`. Because slots
/// process the stem in place in ascending index order, an occupied position's
/// input measurement is exactly the preceding occupied position's output.
pub type PatchEffectSlotObservations =
    [[PatchEffectObservation; MAX_EFFECT_SLOTS]; MAX_ACTIVE_PATCHES];

/// Fixed callback-local measurement around one Patch's post-effect stage.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchEffectObservation {
    patch_id: Option<PatchId>,
    input_rms: f32,
    output_rms: f32,
    difference_rms: f32,
    side_rms: f32,
}

impl PatchEffectObservation {
    pub const EMPTY: Self = Self {
        patch_id: None,
        input_rms: 0.0,
        output_rms: 0.0,
        difference_rms: 0.0,
        side_rms: 0.0,
    };

    pub(crate) fn measured(patch_id: PatchId, before: &[f32], after: &[f32]) -> Self {
        let mut input_energy = 0.0_f64;
        let mut output_energy = 0.0_f64;
        let mut difference_energy = 0.0_f64;
        let mut side_energy = 0.0_f64;
        let mut frames = 0_usize;
        for (input, output) in before.chunks_exact(2).zip(after.chunks_exact(2)) {
            let in_l = finite_sample(input[0]);
            let in_r = finite_sample(input[1]);
            let out_l = finite_sample(output[0]);
            let out_r = finite_sample(output[1]);
            input_energy += f64::from(in_l) * f64::from(in_l) + f64::from(in_r) * f64::from(in_r);
            output_energy +=
                f64::from(out_l) * f64::from(out_l) + f64::from(out_r) * f64::from(out_r);
            let difference_l = out_l - in_l;
            let difference_r = out_r - in_r;
            difference_energy += f64::from(difference_l) * f64::from(difference_l)
                + f64::from(difference_r) * f64::from(difference_r);
            let side = (out_l - out_r) * 0.5;
            side_energy += f64::from(side) * f64::from(side);
            frames += 1;
        }
        let samples = (frames.saturating_mul(2)).max(1) as f64;
        let side_samples = frames.max(1) as f64;
        Self {
            patch_id: Some(patch_id),
            input_rms: finite_measure((input_energy / samples).sqrt() as f32),
            output_rms: finite_measure((output_energy / samples).sqrt() as f32),
            difference_rms: finite_measure((difference_energy / samples).sqrt() as f32),
            side_rms: finite_measure((side_energy / side_samples).sqrt() as f32),
        }
    }

    pub(crate) const fn from_parts(
        patch_id: Option<PatchId>,
        input_rms: f32,
        output_rms: f32,
        difference_rms: f32,
        side_rms: f32,
    ) -> Self {
        Self {
            patch_id,
            input_rms,
            output_rms,
            difference_rms,
            side_rms,
        }
    }

    pub const fn patch_id(self) -> Option<PatchId> {
        self.patch_id
    }

    pub const fn input_rms(self) -> f32 {
        self.input_rms
    }

    pub const fn output_rms(self) -> f32 {
        self.output_rms
    }

    pub const fn difference_rms(self) -> f32 {
        self.difference_rms
    }

    pub const fn side_rms(self) -> f32 {
        self.side_rms
    }
}

fn finite_sample(value: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

fn finite_measure(value: f32) -> f32 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        0.0
    }
}

impl Default for PatchEffectObservation {
    fn default() -> Self {
        Self::EMPTY
    }
}
