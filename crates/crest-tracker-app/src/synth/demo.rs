//! The bundled example is a complete document, admitted by the normal host lifecycle.
use crate::persistence::Document;

pub(super) fn document() -> Result<Document, String> {
    let document: Document =
        serde_json::from_str(include_str!("../../../../assets/window-light.crest-song"))
            .map_err(|error| format!("Bundled example is invalid: {error}"))?;
    document.validate()?;
    Ok(document)
}
