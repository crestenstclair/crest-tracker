use crest_synth::testing::{compare_composition_facts, CompositionFact, ReviewManifest};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[path = "support/phase03_visual_evidence.rs"]
mod phase03_visual_evidence_support;

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/phase03")
}

fn read_json(name: &str) -> Value {
    let bytes = std::fs::read(fixture_root().join(name)).expect("Phase 03 fixture is readable");
    serde_json::from_slice(&bytes).expect("Phase 03 fixture is valid JSON")
}

fn nonempty_array<'a>(value: &'a Value, pointer: &str) -> &'a Vec<Value> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| panic!("{pointer} is a non-empty array"))
}

#[test]
fn composition_ledger_is_bounded_complete_and_matches_exported_frames() {
    let ledger = read_json("composition-ledger.json");
    assert_eq!(ledger.get("schemaVersion").and_then(Value::as_u64), Some(1));
    assert_eq!(
        ledger.get("fileKey").and_then(Value::as_str),
        Some("kdQMw8dYUZtv2UxJPo0sXU")
    );
    assert!(ledger
        .get("retrievedAt")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty()));

    let references = nonempty_array(&ledger, "/references");
    assert_eq!(
        references.len(),
        8,
        "the bounded ledger names all eight live nodes"
    );
    let expected_nodes = [
        "98:2", "39:92", "41:138", "42:3", "48:173", "48:207", "49:3", "95:202",
    ]
    .into_iter()
    .collect::<HashSet<_>>();
    let observed_nodes = references
        .iter()
        .filter_map(|reference| reference.get("nodeId").and_then(Value::as_str))
        .collect::<HashSet<_>>();
    assert_eq!(observed_nodes, expected_nodes);
    for reference in references {
        let relative = reference
            .get("imagePath")
            .and_then(Value::as_str)
            .expect("reference names its committed image");
        let expected_hash = reference
            .get("imageSha256")
            .and_then(Value::as_str)
            .expect("reference names the retrieved image hash");
        let bytes = std::fs::read(fixture_root().join(relative))
            .expect("the exact retrieved Figma export is committed");
        let actual = format!("{:x}", Sha256::digest(bytes));
        assert_eq!(actual, expected_hash);
    }

    let surfaces = nonempty_array(&ledger, "/surfaces");
    assert_eq!(
        surfaces.len(),
        5,
        "the ledger is bounded to Phase 03 surfaces"
    );
    for surface in surfaces {
        for field in [
            "hierarchy",
            "relativeEmphasis",
            "typeRoles",
            "spacingRhythm",
            "flexibleRegionRelationships",
            "stateMarkers",
            "fixtureFacts",
            "productionDistinctions",
        ] {
            assert!(
                surface
                    .get(field)
                    .and_then(Value::as_array)
                    .is_some_and(|values| !values.is_empty()),
                "{} records {field}",
                surface
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("surface")
            );
        }
        assert!(surface
            .get("focusTreatment")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty()));
    }
}

#[test]
fn projection_audit_adds_no_named_capability_or_renderer_state_proposal() {
    let audit = read_json("projection-audit.json");
    assert_eq!(audit.get("schemaVersion").and_then(Value::as_u64), Some(1));
    assert_eq!(
        audit
            .get("prohibitedProposals")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );
    let banned = [
        "plaits",
        "chorus",
        "delay",
        "reverb",
        "six options",
        "eight files",
    ];
    for proposal in nonempty_array(&audit, "/areas")
        .iter()
        .flat_map(|area| area.get("missingGenericFacts").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
    {
        let lower = proposal.to_ascii_lowercase();
        assert!(
            banned.iter().all(|name| !lower.contains(name)),
            "missing fact remains generic: {proposal}"
        );
        assert!(!lower.contains("dom state"));
        assert!(!lower.contains("renderer-owned"));
    }
}

#[test]
fn review_schema_requires_correspondence_and_rejects_fixed_image_gates() {
    let schema = read_json("review-manifest.schema.json");
    let required = nonempty_array(&schema, "/required");
    for field in [
        "schemaVersion",
        "reviewId",
        "surface",
        "figma",
        "native",
        "referenceFacts",
        "nativeFacts",
    ] {
        assert!(required.iter().any(|value| value.as_str() == Some(field)));
    }
    assert_eq!(
        schema.pointer("/properties/coordinateEqualityRequired/const"),
        Some(&Value::Bool(false))
    );
    assert_eq!(
        schema.pointer("/properties/pixelDiffRequired/const"),
        Some(&Value::Bool(false))
    );

    // Keep the public manifest validator and schema joined: an empty object
    // cannot deserialize, while a coordinate gate cannot validate.
    assert!(serde_json::from_value::<ReviewManifest>(serde_json::json!({})).is_err());
}

#[test]
fn relationship_review_ignores_geometry_but_detects_controlled_regression() {
    let reference = CompositionFact {
        role: "mixer.levelFader".to_owned(),
        parent_role: Some("mixer.track".to_owned()),
        order: 1,
        type_role: "Code/Value".to_owned(),
        emphasis: "dominant".to_owned(),
        state_marker: "FOCUSED".to_owned(),
        bounds: None,
    };
    let mut responsive = reference.clone();
    responsive.bounds = Some(crest_synth::testing::CompositionRect {
        x_px: 320.0,
        y_px: 140.0,
        width_px: 48.0,
        height_px: 300.0,
    });
    assert!(compare_composition_facts(std::slice::from_ref(&reference), &[responsive]).is_empty());

    let mut regressed = reference.clone();
    regressed.parent_role = Some("footer".to_owned());
    regressed.state_marker = "RESTING".to_owned();
    let discrepancy_fields = compare_composition_facts(&[reference], &[regressed])
        .into_iter()
        .map(|issue| issue.field)
        .collect::<HashSet<_>>();
    assert_eq!(
        discrepancy_fields,
        ["parentRole".to_owned(), "stateMarker".to_owned()]
            .into_iter()
            .collect()
    );
}
