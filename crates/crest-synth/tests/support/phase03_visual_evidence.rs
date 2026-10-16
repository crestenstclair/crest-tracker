use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Joins exact semantic-document bytes with one production-page observation.
/// Construction is strict: missing paint, scale, focus, subject,
/// compatibility, reachability, overlap, overflow, or event-count evidence is
/// a typed test failure rather than an inferred pass.
pub fn native_responsive_observation(
    document_json: &str,
    page: &Value,
    input_event_count: u64,
    reducer_application_count: u64,
) -> Result<Value, String> {
    let document: Value = serde_json::from_str(document_json)
        .map_err(|error| format!("semantic document is valid JSON: {error}"))?;
    let required = |pointer: &str| {
        page.pointer(pointer)
            .cloned()
            .ok_or_else(|| format!("native observation is missing {pointer}"))
    };
    let generation = required("/generation")?;
    let state_hash = required("/stateHash")?;
    if Some(&generation) != document.get("generation") {
        return Err("native observation generation does not match the document".to_owned());
    }
    if Some(&state_hash) != document.get("stateHash") {
        return Err("native observation stateHash does not match the document".to_owned());
    }
    let paint = required("/paintAcknowledgment")?;
    for field in [
        "generation",
        "stateHash",
        "context",
        "activeSurface",
        "focusPath",
        "interactionMode",
    ] {
        if paint.get(field) != document.get(field) {
            return Err(format!(
                "paint acknowledgement does not copy {field} from the document"
            ));
        }
    }
    let device_scale = required("/deviceScale")?;
    if !device_scale
        .as_f64()
        .is_some_and(|scale| scale.is_finite() && scale > 0.0)
    {
        return Err("native observation deviceScale is invalid".to_owned());
    }
    if paint.get("deviceScale") != Some(&device_scale) {
        return Err("paint acknowledgement deviceScale does not match the observation".to_owned());
    }
    let context = required("/semanticEvidence/context")?;
    if Some(&context) != document.get("context") {
        return Err("native observation context does not match the document".to_owned());
    }
    let active_surface = required("/semanticEvidence/activeSurface")?;
    if Some(&active_surface) != document.get("activeSurface") {
        return Err("native observation active surface does not match the document".to_owned());
    }
    let focus = required("/semanticEvidence/focusPath")?;
    if Some(&focus) != document.get("focusPath") {
        return Err("native observation focus does not match the document".to_owned());
    }
    let subject = required("/semanticEvidence/subject")?;
    let expected_subject = document
        .get("surfaces")
        .and_then(Value::as_array)
        .and_then(|surfaces| {
            surfaces
                .iter()
                .find(|surface| surface.get("id") == Some(&active_surface))
        })
        .and_then(|surface| surface.get("summary"))
        .ok_or_else(|| "semantic document has no active-surface summary".to_owned())?;
    if &subject != expected_subject {
        return Err("native observation subject does not match the active surface".to_owned());
    }
    let return_identity = required("/semanticEvidence/returnIdentity")?;
    if Some(&return_identity) != document.get("returnPath") {
        return Err("native observation return identity does not match the document".to_owned());
    }
    let valid_actions = required("/semanticEvidence/validActions")?;
    if Some(&valid_actions) != document.get("validActions") {
        return Err("native observation valid actions do not match the document".to_owned());
    }
    let compatibility = required("/semanticEvidence/observationCompatibility")?;
    if compatibility.get("parameterGeneration") != document.get("generation") {
        return Err("native observation compatibility generation is stale".to_owned());
    }
    let expected_graph_revision = document
        .pointer("/status/graphRevision")
        .cloned()
        .unwrap_or(Value::Null);
    if compatibility.get("activeGraphRevision") != Some(&expected_graph_revision) {
        return Err("native observation graph compatibility is stale".to_owned());
    }
    let minimum_target = required("/minimumTarget")?;
    if !minimum_target
        .get("floorPx")
        .and_then(Value::as_f64)
        .is_some_and(|floor| floor.is_finite() && floor > 0.0)
        || minimum_target.get("compliant").and_then(Value::as_bool) != Some(true)
        || minimum_target.get("failureCount").and_then(Value::as_u64) != Some(0)
    {
        return Err("native observation reports a minimum-target failure".to_owned());
    }
    let required_overlap_px = required("/layout/workspaceInspectorOverlapPx")?;
    let horizontal_overflow_px = required("/layout/horizontalOverflowPx")?;
    let scroll_endpoints = required("/reachability")?;
    if required_overlap_px.as_u64() != Some(0) {
        return Err("native observation reports required-content overlap".to_owned());
    }
    if horizontal_overflow_px.as_u64() != Some(0) {
        return Err("native observation reports document horizontal overflow".to_owned());
    }
    if scroll_endpoints
        .get("focusedVisible")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Err("native observation focus is not reachable".to_owned());
    }
    for region in ["workspace", "inspector"] {
        let endpoints = scroll_endpoints
            .get(region)
            .ok_or_else(|| format!("native observation has no {region} scroll endpoints"))?;
        for field in [
            "startReachable",
            "endReachable",
            "firstTargetReachable",
            "lastTargetReachable",
        ] {
            if endpoints.get(field).and_then(Value::as_bool) != Some(true) {
                return Err(format!(
                    "native observation {region} endpoint {field} is unreachable"
                ));
            }
        }
    }
    let viewport = required("/viewport")?;
    let semantic_document_sha256 = format!("{:x}", Sha256::digest(document_json.as_bytes()));
    Ok(json!({
        "semanticDocumentSha256": semantic_document_sha256,
        "generation": generation,
        "stateHash": state_hash,
        "focus": focus,
        "subject": subject,
        "returnIdentity": return_identity,
        "validActions": valid_actions,
        "deviceScale": device_scale,
        "minimumTarget": minimum_target,
        "requiredOverlapPx": required_overlap_px,
        "horizontalOverflowPx": horizontal_overflow_px,
        "scrollEndpoints": scroll_endpoints,
        "observationCompatibility": compatibility,
        "inputEventCount": input_event_count,
        "reducerApplicationCount": reducer_application_count,
        "viewport": viewport,
        "paintAcknowledgement": paint
    }))
}

#[cfg(test)]
#[allow(dead_code)] // This support module is also included by a harness=false native witness.
mod tests {
    use super::*;

    fn document() -> String {
        json!({
            "generation": 7,
            "stateHash": "state-7",
            "context": "patch",
            "activeSurface": "patchDetail",
            "focusPath": {"surface":"patchDetail"},
            "interactionMode": "navigate",
            "returnPath": null,
            "validActions": [],
            "status": {"graphRevision": 3},
            "surfaces": [{
                "id": "patchDetail",
                "summary": {"kind":"patchDetail"}
            }]
        })
        .to_string()
    }

    fn page() -> Value {
        json!({
            "generation": 7,
            "stateHash": "state-7",
            "deviceScale": 2.0,
            "viewport": {"widthPx": 1280, "heightPx": 800},
            "semanticEvidence": {
                "context": "patch",
                "activeSurface": "patchDetail",
                "focusPath": {"surface":"patchDetail"},
                "subject": {"kind":"patchDetail"},
                "returnIdentity": null,
                "validActions": [],
                "observationCompatibility": {
                    "parameterGeneration":7,
                    "activeGraphRevision":3
                }
            },
            "minimumTarget": {"floorPx":48,"compliant":true,"failureCount":0},
            "layout": {"workspaceInspectorOverlapPx":0,"horizontalOverflowPx":0},
            "reachability": {
                "focusedVisible": true,
                "workspace": {
                    "startReachable":true,
                    "endReachable":true,
                    "firstTargetReachable":true,
                    "lastTargetReachable":true
                },
                "inspector": {
                    "startReachable":true,
                    "endReachable":true,
                    "firstTargetReachable":true,
                    "lastTargetReachable":true
                }
            },
            "paintAcknowledgment": {
                "generation":7,
                "stateHash":"state-7",
                "context":"patch",
                "activeSurface":"patchDetail",
                "focusPath":{"surface":"patchDetail"},
                "interactionMode":"navigate",
                "deviceScale":2.0
            }
        })
    }

    #[test]
    fn complete_observation_is_correlated_and_event_counted() {
        let evidence = native_responsive_observation(&document(), &page(), 0, 0)
            .expect("complete evidence is accepted");
        assert_eq!(
            evidence.get("inputEventCount").and_then(Value::as_u64),
            Some(0)
        );
        assert_eq!(
            evidence
                .get("reducerApplicationCount")
                .and_then(Value::as_u64),
            Some(0)
        );
        assert!(evidence
            .get("semanticDocumentSha256")
            .and_then(Value::as_str)
            .is_some_and(|hash| hash.len() == 64));
    }

    #[test]
    fn malformed_or_incomplete_evidence_is_rejected() {
        for pointer in [
            "/deviceScale",
            "/semanticEvidence/context",
            "/semanticEvidence/activeSurface",
            "/semanticEvidence/subject",
            "/semanticEvidence/observationCompatibility",
            "/minimumTarget",
            "/reachability",
            "/paintAcknowledgment",
        ] {
            let mut incomplete = page();
            incomplete
                .pointer_mut(pointer)
                .expect("controlled field exists")
                .take();
            assert!(
                native_responsive_observation(&document(), &incomplete, 0, 0).is_err(),
                "missing {pointer} is rejected"
            );
        }
        let mut overlap = page();
        overlap["minimumTarget"]["compliant"] = Value::Bool(false);
        assert!(native_responsive_observation(&document(), &overlap, 0, 0).is_err());

        let mut mismatched = page();
        mismatched["semanticEvidence"]["validActions"] = json!([{"label":"invented"}]);
        assert!(native_responsive_observation(&document(), &mismatched, 0, 0).is_err());
    }
}
