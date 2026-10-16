use serde_json::Value;

/// Capture this witness's WKWebView directly. Desktop screencapture may be
/// unavailable even when the owned native window paints correctly. WebKit's
/// snapshot API captures only that rendered view, with no display permission
/// or alternate renderer, and reports failure rather than a fictitious path.
#[cfg(target_os = "macos")]
pub fn capture(window: &tauri::WebviewWindow, path: &std::path::Path) -> Result<(), String> {
    use block2::RcBlock;
    use objc2::{msg_send, runtime::AnyObject};
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage};
    use objc2_foundation::{NSDictionary, NSError};
    use std::sync::mpsc;
    use std::time::Duration;

    let (sender, receiver) = mpsc::channel();
    window
        .with_webview(move |webview| {
            let completion = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
                let result = (|| {
                    // SAFETY: WebKit invokes this block with borrowed, nullable
                    // NSImage/NSError objects valid for the callback's duration.
                    let image = unsafe { image.as_ref() }.ok_or_else(|| {
                        if let Some(error) = unsafe { error.as_ref() } {
                            format!("native snapshot failed: {}", error.localizedDescription())
                        } else {
                            "native snapshot returned no image".to_owned()
                        }
                    })?;
                    let tiff = image.TIFFRepresentation().ok_or("snapshot has no bitmap")?;
                    let bitmap = NSBitmapImageRep::imageRepWithData(&tiff)
                        .ok_or("snapshot bitmap could not be decoded")?;
                    // SAFETY: The empty properties dictionary supplies no values
                    // of an incorrect type. All conversion is on the UI thread.
                    let png = unsafe {
                        bitmap.representationUsingType_properties(
                            NSBitmapImageFileType::PNG,
                            &NSDictionary::new(),
                        )
                    }
                    .ok_or("snapshot could not be encoded as PNG")?;
                    Ok::<_, String>(png.to_vec())
                })();
                let _ = sender.send(result);
            });
            // SAFETY: Tauri owns a live WKWebView and invokes with_webview on
            // the UI thread. WebKit copies the completion block for async use;
            // nil configuration requests the currently visible webview bounds.
            unsafe {
                let view = &*webview.inner().cast::<AnyObject>();
                let _: () = msg_send![view,
                    takeSnapshotWithConfiguration: None::<&AnyObject>,
                    completionHandler: &*completion
                ];
            }
        })
        .map_err(|error| error.to_string())?;
    let bytes = receiver
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| "native snapshot did not finish within 10s".to_owned())??;
    std::fs::write(path, bytes).map_err(|error| error.to_string())
}

#[cfg(target_os = "linux")]
pub fn capture(window: &tauri::WebviewWindow, path: &std::path::Path) -> Result<(), String> {
    use webkit2gtk::WebViewExt;
    let (sender, receiver) = std::sync::mpsc::channel();
    window
        .with_webview(move |webview| {
            webview.inner().snapshot(
                webkit2gtk::SnapshotRegion::Visible,
                webkit2gtk::SnapshotOptions::NONE,
                None::<&gtk::gio::Cancellable>,
                move |result| {
                    let result = result
                        .map_err(|error| error.to_string())
                        .and_then(|surface| {
                            let mut bytes = Vec::new();
                            surface
                                .write_to_png(&mut bytes)
                                .map_err(|error| error.to_string())?;
                            Ok(bytes)
                        });
                    let _ = sender.send(result);
                },
            );
        })
        .map_err(|error| error.to_string())?;
    let bytes = receiver
        .recv_timeout(std::time::Duration::from_secs(10))
        .map_err(|_| "native snapshot did not finish within 10s".to_owned())??;
    std::fs::write(path, bytes).map_err(|error| error.to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn capture(_: &tauri::WebviewWindow, _: &std::path::Path) -> Result<(), String> {
    Err("Sample native capture requires WKWebView on macOS".to_owned())
}

pub fn assert_waveform(detail: &Value, painted: &Value, data: &Value, label: &str) {
    use crest_synth::shell::tokens::TypeStyle;
    let scale = detail["textScale"].as_f64().unwrap();
    for (role, style) in [
        ("assetLabel", TypeStyle::LabelControl),
        ("assetPath", TypeStyle::CodeValue),
        ("subject", TypeStyle::HeadingSection),
        ("sectionLabel", TypeStyle::LabelControl),
        ("parameterLabel", TypeStyle::LabelControl),
        ("parameterValue", TypeStyle::CodeValue),
        ("status", TypeStyle::InstructionHint),
    ] {
        let observed = &detail["typography"][role];
        let expected = style.metrics();
        assert!(observed["font"].as_str().unwrap().contains("Azeret Mono"));
        assert!(
            (observed["size"].as_f64().unwrap() - f64::from(expected.size_px) * scale).abs() < 0.01,
            "{label}: {role} resolves the authored type role"
        );
    }
    assert_eq!(
        detail["typography"]["subject"]["color"],
        "rgb(255, 180, 84)"
    );
    let waveform = &painted["waveform"];
    assert!(
        waveform.is_object(),
        "{label}: waveform has native observations"
    );
    assert_eq!(waveform["status"], data["status"]);
    assert_eq!(
        waveform["asset"],
        data["asset"]["locator"].as_str().unwrap_or("EMPTY")
    );
    let hierarchy = detail["hierarchy"].as_array().unwrap();
    let leading = usize::from(label.contains("patch-drum-rack"));
    if leading > 0 {
        assert_eq!(
            hierarchy[0], "detail-section",
            "{label}: pad selector leads the Sample layout"
        );
    }
    assert_eq!(
        &hierarchy[leading..leading + 2],
        &["detail-asset", "waveform"],
        "{label}: file and waveform follow leading controls"
    );
    assert!(
        hierarchy.iter().position(|v| v == "waveform").unwrap()
            < hierarchy
                .iter()
                .position(|v| v == "detail-sections")
                .unwrap()
    );
    assert!(waveform["labelFont"]
        .as_str()
        .unwrap()
        .contains("Azeret Mono"));
    let pairs = data["pairs"].as_array().unwrap();
    let bars = waveform["bars"].as_array().unwrap();
    let landmarks = waveform["landmarks"].as_array().unwrap();
    if pairs.is_empty() {
        assert_eq!(waveform["state"], "unavailable");
        assert_eq!(waveform["unavailableBorder"], "dashed");
        assert!(
            bars.is_empty() && landmarks.is_empty(),
            "{label}: unavailable data paints no fabricated geometry"
        );
        return;
    }
    assert_eq!(waveform["state"], "available");
    let stride = pairs.len().div_ceil(192);
    assert_eq!(bars.len(), pairs.len().div_ceil(stride));
    let track = &waveform["trackBounds"];
    let track_height = track["heightPx"].as_f64().unwrap();
    assert!(track_height > 0.0);
    for (bar, bin) in bars.iter().zip(pairs.chunks(stride)) {
        let low = bin
            .iter()
            .flat_map(|pair| {
                [
                    pair["leftMin"].as_f64().unwrap(),
                    pair["rightMin"].as_f64().unwrap(),
                ]
            })
            .reduce(f64::min)
            .unwrap();
        let high = bin
            .iter()
            .flat_map(|pair| {
                [
                    pair["leftMax"].as_f64().unwrap(),
                    pair["rightMax"].as_f64().unwrap(),
                ]
            })
            .reduce(f64::max)
            .unwrap();
        let expected_height = (high - low) / 2.0;
        let expected_top = (1.0 - high) / 2.0;
        assert!(
            (bar["height"].as_f64().unwrap() - expected_height).abs() < 0.000002,
            "{label}: both channels contribute"
        );
        assert!((bar["top"].as_f64().unwrap() - expected_top).abs() < 0.000002);
        assert!(
            (bar["bounds"]["heightPx"].as_f64().unwrap() - expected_height * track_height).abs()
                <= 1.0,
            "{label}: waveform geometry is actually painted under the production CSP"
        );
        assert_eq!(
            bar["color"], "rgb(255, 180, 84)",
            "{label}: authored Sample tone"
        );
    }
    let expected_landmarks = data["landmarks"].as_array().unwrap();
    assert_eq!(landmarks.len(), expected_landmarks.len());
    for (marker, expected) in landmarks.iter().zip(expected_landmarks) {
        assert_eq!(marker["role"], expected["role"]);
        assert!(
            (marker["position"].as_f64().unwrap()
                - expected["normalizedPosition"].as_f64().unwrap())
            .abs()
                < 0.000002
        );
        assert!(!marker["label"].as_str().unwrap().is_empty());
        assert!(marker["bounds"]["widthPx"].as_f64().unwrap() > 0.0);
        let role = expected["role"].as_str().unwrap();
        let expected_border = match role {
            "loopStart" | "loopEnd" => "dashed",
            "playbackEnd" => "double",
            _ => "solid",
        };
        assert_eq!(
            marker["border"], expected_border,
            "{label}: landmark roles have distinct shapes"
        );
    }
}
