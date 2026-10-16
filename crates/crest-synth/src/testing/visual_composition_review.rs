//! Deterministic, relationship-based review evidence for native/Figma pairs.
//!
//! This module deliberately compares semantic composition facts, not pixels or
//! coordinates. A responsive native capture may place a region at different
//! coordinates from a static Figma fixture while preserving the authored
//! hierarchy, type role, reading order, and state treatment. Geometry remains
//! attached to each capture so reviewers can inspect clipping and reflow, but
//! it is never an equality input.

use core::fmt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Optional capture geometry retained for annotation, never comparison.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompositionRect {
    pub x_px: f32,
    pub y_px: f32,
    pub width_px: f32,
    pub height_px: f32,
}

/// One authored or native composition fact, keyed by a stable generic role.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompositionFact {
    pub role: String,
    pub parent_role: Option<String>,
    pub order: u32,
    pub type_role: String,
    pub emphasis: String,
    pub state_marker: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<CompositionRect>,
}

/// The reference image identity carried by every review manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormativeCaptureIdentity {
    pub file_key: String,
    pub node_id: String,
    pub retrieved_at: String,
    pub image_path: String,
    pub image_sha256: String,
}

/// The production-native identity carried by every review manifest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeCaptureIdentity {
    pub capture_path: String,
    pub observed_at: String,
    pub window_condition: String,
    pub viewport_width_px: f32,
    pub viewport_height_px: f32,
    pub device_scale: f32,
    pub text_scale: f32,
    pub fixture_identity: String,
    pub semantic_document_sha256: String,
    pub paint_acknowledgement: serde_json::Value,
}

/// One side of a deterministic composition review.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompositionReviewCapture {
    pub image_path: String,
    pub facts: Vec<CompositionFact>,
}

/// A meaningful hierarchy, type, emphasis, ordering, or state mismatch.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompositionDiscrepancy {
    pub role: String,
    pub field: String,
    pub expected: String,
    pub observed: String,
}

/// A complete review manifest joining design and production paint identity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewManifest {
    pub schema_version: u32,
    pub review_id: String,
    pub surface: String,
    pub figma: NormativeCaptureIdentity,
    pub native: NativeCaptureIdentity,
    pub reference_facts: Vec<CompositionFact>,
    pub native_facts: Vec<CompositionFact>,
    pub coordinate_equality_required: bool,
    pub pixel_diff_required: bool,
}

/// Why incomplete review evidence was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewManifestError {
    missing_or_invalid: Vec<&'static str>,
}

impl ReviewManifestError {
    pub fn fields(&self) -> &[&'static str] {
        &self.missing_or_invalid
    }
}

impl fmt::Display for ReviewManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "review manifest has missing or invalid fields: {}",
            self.missing_or_invalid.join(", ")
        )
    }
}

impl std::error::Error for ReviewManifestError {}

fn nonempty(value: &str) -> bool {
    !value.trim().is_empty()
}

fn sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn acknowledgement_complete(value: &serde_json::Value) -> bool {
    value
        .get("generation")
        .and_then(serde_json::Value::as_u64)
        .is_some()
        && value
            .get("stateHash")
            .and_then(serde_json::Value::as_str)
            .is_some_and(nonempty)
        && value
            .get("activeSurface")
            .and_then(serde_json::Value::as_str)
            .is_some_and(nonempty)
        && value.get("focusPath").is_some()
}

impl ReviewManifest {
    /// Refuses a manifest that cannot identify both sources or correlate the
    /// native capture with a production semantic document and paint ack.
    pub fn validate(&self) -> Result<(), ReviewManifestError> {
        let mut invalid = Vec::new();
        if self.schema_version != 1 {
            invalid.push("schemaVersion");
        }
        for (name, value) in [
            ("reviewId", self.review_id.as_str()),
            ("surface", self.surface.as_str()),
            ("figma.fileKey", self.figma.file_key.as_str()),
            ("figma.nodeId", self.figma.node_id.as_str()),
            ("figma.retrievedAt", self.figma.retrieved_at.as_str()),
            ("figma.imagePath", self.figma.image_path.as_str()),
            ("native.capturePath", self.native.capture_path.as_str()),
            ("native.observedAt", self.native.observed_at.as_str()),
            (
                "native.windowCondition",
                self.native.window_condition.as_str(),
            ),
            (
                "native.fixtureIdentity",
                self.native.fixture_identity.as_str(),
            ),
        ] {
            if !nonempty(value) {
                invalid.push(name);
            }
        }
        if !sha256(&self.figma.image_sha256) {
            invalid.push("figma.imageSha256");
        }
        if !sha256(&self.native.semantic_document_sha256) {
            invalid.push("native.semanticDocumentSha256");
        }
        if !self.native.viewport_width_px.is_finite() || self.native.viewport_width_px <= 0.0 {
            invalid.push("native.viewportWidthPx");
        }
        if !self.native.viewport_height_px.is_finite() || self.native.viewport_height_px <= 0.0 {
            invalid.push("native.viewportHeightPx");
        }
        if !self.native.device_scale.is_finite() || self.native.device_scale <= 0.0 {
            invalid.push("native.deviceScale");
        }
        if !self.native.text_scale.is_finite() || self.native.text_scale <= 0.0 {
            invalid.push("native.textScale");
        }
        if !acknowledgement_complete(&self.native.paint_acknowledgement) {
            invalid.push("native.paintAcknowledgement");
        }
        if self.reference_facts.is_empty() {
            invalid.push("referenceFacts");
        }
        if self.native_facts.is_empty() {
            invalid.push("nativeFacts");
        }
        if self.coordinate_equality_required {
            invalid.push("coordinateEqualityRequired");
        }
        if self.pixel_diff_required {
            invalid.push("pixelDiffRequired");
        }
        if invalid.is_empty() {
            Ok(())
        } else {
            Err(ReviewManifestError {
                missing_or_invalid: invalid,
            })
        }
    }
}

/// Compares generic relationship facts and deliberately ignores `bounds`.
pub fn compare_composition_facts(
    reference: &[CompositionFact],
    native: &[CompositionFact],
) -> Vec<CompositionDiscrepancy> {
    let native_by_role = native
        .iter()
        .map(|fact| (fact.role.as_str(), fact))
        .collect::<BTreeMap<_, _>>();
    let mut discrepancies = Vec::new();
    for expected in reference {
        let Some(observed) = native_by_role.get(expected.role.as_str()).copied() else {
            discrepancies.push(CompositionDiscrepancy {
                role: expected.role.clone(),
                field: "presence".to_owned(),
                expected: "present".to_owned(),
                observed: "missing".to_owned(),
            });
            continue;
        };
        let pairs = [
            (
                "parentRole",
                expected.parent_role.clone().unwrap_or_default(),
                observed.parent_role.clone().unwrap_or_default(),
            ),
            (
                "order",
                expected.order.to_string(),
                observed.order.to_string(),
            ),
            (
                "typeRole",
                expected.type_role.clone(),
                observed.type_role.clone(),
            ),
            (
                "emphasis",
                expected.emphasis.clone(),
                observed.emphasis.clone(),
            ),
            (
                "stateMarker",
                expected.state_marker.clone(),
                observed.state_marker.clone(),
            ),
        ];
        for (field, wanted, actual) in pairs {
            if wanted != actual {
                discrepancies.push(CompositionDiscrepancy {
                    role: expected.role.clone(),
                    field: field.to_owned(),
                    expected: wanted,
                    observed: actual,
                });
            }
        }
    }
    discrepancies
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Produces stable side-by-side review HTML with meaningful annotations.
pub fn render_composition_review_html(
    title: &str,
    reference: &CompositionReviewCapture,
    native: &CompositionReviewCapture,
) -> String {
    let discrepancies = compare_composition_facts(&reference.facts, &native.facts);
    let annotations = if discrepancies.is_empty() {
        "<li>NO MEANINGFUL DISCREPANCIES</li>".to_owned()
    } else {
        discrepancies
            .iter()
            .map(|issue| {
                format!(
                    "<li><code>{}</code> {}: expected <b>{}</b>, observed <b>{}</b></li>",
                    escape_html(&issue.role),
                    escape_html(&issue.field),
                    escape_html(&issue.expected),
                    escape_html(&issue.observed)
                )
            })
            .collect::<Vec<_>>()
            .join("")
    };
    format!(
        "<!doctype html><meta charset=\"utf-8\"><title>{title}</title>\
<style>body{{font-family:monospace;background:{canvas};color:{text}}}main{{display:grid;grid-template-columns:1fr 1fr;gap:24px}}img{{max-width:100%;border:1px solid {border}}}li{{margin:8px 0}}</style>\
<h1>{title}</h1><main><figure><figcaption>FIGMA REFERENCE</figcaption><img src=\"{reference_image}\"></figure>\
<figure><figcaption>PRODUCTION NATIVE</figcaption><img src=\"{native_image}\"></figure></main>\
<h2>MEANINGFUL DISCREPANCIES</h2><ul>{annotations}</ul>",
        canvas = color_css(crate::shell::tokens::SemanticColor::BgCanvas),
        text = color_css(crate::shell::tokens::SemanticColor::TextPrimary),
        border = color_css(crate::shell::tokens::SemanticColor::BorderStrong),
        title = escape_html(title),
        reference_image = escape_html(&reference.image_path),
        native_image = escape_html(&native.image_path),
    )
}

fn color_css(role: crate::shell::tokens::SemanticColor) -> String {
    let color = role.resolve();
    format!("rgb({}, {}, {})", color.r(), color.g(), color.b())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fact(role: &str, parent: Option<&str>, order: u32, state: &str) -> CompositionFact {
        CompositionFact {
            role: role.to_owned(),
            parent_role: parent.map(str::to_owned),
            order,
            type_role: "Label/Control".to_owned(),
            emphasis: "secondary".to_owned(),
            state_marker: state.to_owned(),
            bounds: None,
        }
    }

    #[test]
    fn coordinate_only_reflow_is_not_a_discrepancy() {
        let mut expected = fact("option.row", Some("option.list"), 1, "FOCUSED");
        expected.bounds = Some(CompositionRect {
            x_px: 24.0,
            y_px: 96.0,
            width_px: 872.0,
            height_px: 48.0,
        });
        let mut reflowed = expected.clone();
        reflowed.bounds = Some(CompositionRect {
            x_px: 16.0,
            y_px: 140.0,
            width_px: 560.0,
            height_px: 72.0,
        });
        assert!(compare_composition_facts(&[expected], &[reflowed]).is_empty());
    }

    #[test]
    fn hierarchy_and_state_regressions_are_annotated_deterministically() {
        let expected = fact("option.row", Some("option.list"), 1, "FOCUSED");
        let observed = fact("option.row", Some("footer"), 1, "RESTING");
        let discrepancies = compare_composition_facts(
            std::slice::from_ref(&expected),
            std::slice::from_ref(&observed),
        );
        assert_eq!(discrepancies.len(), 2);
        let reference = CompositionReviewCapture {
            image_path: "figma.png".to_owned(),
            facts: vec![expected],
        };
        let native = CompositionReviewCapture {
            image_path: "native.png".to_owned(),
            facts: vec![observed],
        };
        let first = render_composition_review_html("Option Review", &reference, &native);
        let second = render_composition_review_html("Option Review", &reference, &native);
        assert_eq!(first, second);
        assert!(first.contains("parentRole"));
        assert!(first.contains("stateMarker"));
        assert!(first.contains("grid-template-columns:1fr 1fr"));
    }

    #[test]
    fn manifest_rejects_missing_identity_or_correspondence_fields() {
        let valid = ReviewManifest {
            schema_version: 1,
            review_id: "sample-detail-wide".to_owned(),
            surface: "sampleDetail".to_owned(),
            figma: NormativeCaptureIdentity {
                file_key: "file".to_owned(),
                node_id: "39:92".to_owned(),
                retrieved_at: "2026-08-29T15:13:45Z".to_owned(),
                image_path: "figma/sample-detail.png".to_owned(),
                image_sha256: "a".repeat(64),
            },
            native: NativeCaptureIdentity {
                capture_path: "native/sample-detail.png".to_owned(),
                observed_at: "2026-08-29T16:00:00Z".to_owned(),
                window_condition: "expanded".to_owned(),
                viewport_width_px: 1_920.0,
                viewport_height_px: 1_080.0,
                device_scale: 2.0,
                text_scale: 1.0,
                fixture_identity: "sample-ready-long-asset".to_owned(),
                semantic_document_sha256: "b".repeat(64),
                paint_acknowledgement: json!({
                    "generation": 42,
                    "stateHash": "state-42",
                    "activeSurface": "patchDetail",
                    "focusPath": {"surface": "patchDetail"}
                }),
            },
            reference_facts: vec![fact("detail.header", None, 0, "READY")],
            native_facts: vec![fact("detail.header", None, 0, "READY")],
            coordinate_equality_required: false,
            pixel_diff_required: false,
        };
        valid.validate().expect("complete manifest is accepted");

        let mut missing_node = valid.clone();
        missing_node.figma.node_id.clear();
        assert!(missing_node
            .validate()
            .unwrap_err()
            .fields()
            .contains(&"figma.nodeId"));

        let mut missing_hash = valid.clone();
        missing_hash.native.semantic_document_sha256.clear();
        assert!(missing_hash
            .validate()
            .unwrap_err()
            .fields()
            .contains(&"native.semanticDocumentSha256"));

        let mut missing_ack = valid.clone();
        missing_ack.native.paint_acknowledgement = json!({"generation": 42});
        assert!(missing_ack
            .validate()
            .unwrap_err()
            .fields()
            .contains(&"native.paintAcknowledgement"));

        let mut fixed_resolution_gate = valid;
        fixed_resolution_gate.coordinate_equality_required = true;
        assert!(fixed_resolution_gate.validate().is_err());
    }
}
