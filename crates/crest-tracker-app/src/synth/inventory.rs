//! Export the same canonical schemas and registration checks as Synth startup.
use crest_synth::adapter::{
    production_effects::{production_effect_preparers, production_effect_providers},
    production_instruments::{production_instrument_preparers, production_instrument_providers},
};
use crest_synth::synth::{compose_effect_registry, compose_instrument_registry};
use serde_json::{json, Value};

pub(super) fn capture() -> Result<Value, Box<dyn std::error::Error>> {
    let instruments = compose_instrument_registry(
        &production_instrument_providers()?,
        &production_instrument_preparers()?,
    )?;
    let effects = compose_effect_registry(
        &production_effect_providers()?,
        &production_effect_preparers()?,
    )?;
    Ok(json!({
        "schema_version":1,
        "scope":"synth_production_capability_inventory",
        "registration_validated":true,
        "instruments":instruments.descriptors(),
        "effects":effects.descriptors(),
        "limitations":"Canonical production metadata and provider/preparer registration; not native workflow, asset import, audible rendering or physical-device acceptance."
    }))
}
