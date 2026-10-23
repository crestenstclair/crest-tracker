// crest-synth projection page.
//
// PURE RENDER. `render(model)` rebuilds the five shell bands from one
// deserialized SemanticGraphicalViewModel document and nothing else: no
// Date.now, no Math.random, no accumulated state, no incidental
// iteration-order dependence (every walk follows the document's own array
// order). The same document always paints the same DOM. Both declared
// top-level contexts render here — MIXER as the sixteen-column bank and PATCH
// as its projected Overview sections — through the same shared
// structural bands (context line, identity header, workspace scaffold,
// persistent side region, footer); nothing forks the schema and no field is
// invented.
//
// The presentation-only carve-outs are the latest observation channels.
// crest://meters repaints only meter elements when its generations match the
// document on screen; crest://midi-activity repaints only the active MIDI
// inspector when its connection revision matches. Missing or stale frames
// render explicit resting states and never mutate the projected model.
//
// This page registers no key handler and captures no input; keys are
// captured Rust-side (WP01/WP02 boundary). Listener glue (tauri events →
// parse → render → paint ack) is kept separate from the pure function so the
// acceptance harness can drive `render`/`renderObservation` headlessly.
"use strict";

(function pageModule() {
  var PROJECTION_EVENT = "crest://projection";
  var METER_EVENT = "crest://meters";
  var MIDI_ACTIVITY_EVENT = "crest://midi-activity";
  var SESSION_DOCUMENT_EVENT = "crest://session-document";
  var latestSessionDocument = null;
  var PAINTED_EVENT = "crest://painted";
  var READY_EVENT = "crest://ready";
  var RENDER_ERROR_EVENT = "crest://render-error";

  // Explicit-unavailability mark: a declared structure with no view data
  // behind it says so; it is never painted with a representative value and
  // never dressed as the resting state (MixerTrackColumnStructure rule).
  var UNAVAILABLE = "UNAVAILABLE";

  // The row-level unavailable mark the pre-cutover shipped rows painted
  // (parameter_row::UNAVAILABLE_MARK): a designed row whose value data did
  // not arrive reads "--" where its value would be.
  var UNAVAILABLE_MARK = "--";

  // The authored hint separator (primitives::hint::HINT_SEPARATOR).
  var HINT_SEPARATOR = " · ";

  // The declared column anatomy, closed and ordered. Rendering walks exactly this
  // list; the observation reports it per column so a lost or reordered
  // structure is measurable.
  var COLUMN_ANATOMY = [
    "TrackHeader",
    "LevelFader",
    "LevelReadout",
    "PanReadout",
    "StateLine",
  ];

  // The authored range separator, the same one the Inspector's own range
  // reading already uses ("LEVEL 00 — 7F").
  var RANGE_SEPARATOR = " — ";

  // The non-color mark a capability-declared read-only row carries. The row
  // also takes a dashed keyline in page.css, so the declaration reads in text
  // and in shape, not in color alone (ComponentState rule).
  var READ_ONLY_MARK = "READ-ONLY";

  // ---- pure helpers ------------------------------------------------------

  function escapeHtml(text) {
    return String(text).replace(/[&<>"']/g, function (ch) {
      return {
        "&": "&amp;",
        "<": "&lt;",
        ">": "&gt;",
        '"': "&quot;",
        "'": "&#39;",
      }[ch];
    });
  }

  // Unwraps the nested value shapes the document serializes:
  // {kind:"scalar", value:0.5} and {kind:"parameter", value:{kind:"toggle",
  // value:false}} both resolve to their innermost primitive.
  function innerValue(control) {
    var value = control && control.value;
    var guard = 0;
    while (value !== null && typeof value === "object" && guard < 8) {
      value = value.value;
      guard += 1;
    }
    return value;
  }

  function toggleOn(control) {
    var value = innerValue(control);
    return value === true || value === 1;
  }

  // Normalized position of a continuous control inside its declared range.
  function fraction(control) {
    var range = control && control.numericRange;
    var value = innerValue(control);
    if (!range || typeof value !== "number" || range.maximum === range.minimum) {
      return 0;
    }
    var f = (value - range.minimum) / (range.maximum - range.minimum);
    return Math.min(1, Math.max(0, f));
  }

  // Where a row's value sits inside its declared bounds, or null when the
  // document carries no bounds or no numeric value — a row drawn half-full
  // because nobody said how full it is would be a claim the projection never
  // made (parameter_row::position_fraction).
  function positionFraction(control) {
    var range = control && control.numericRange;
    var value = innerValue(control);
    if (!range || typeof value !== "number" || range.maximum === range.minimum) {
      return null;
    }
    return fraction(control);
  }

  // The bound MidiHexadecimal form: (v-min)/(max-min)*127 as two uppercase
  // hex digits. Bound to the LevelReadout presentation (and the Inspector's
  // reading of the same focused level).
  function midiHex(control) {
    var scaled = Math.round(fraction(control) * 127);
    var hex = scaled.toString(16).toUpperCase();
    return hex.length < 2 ? "0" + hex : hex;
  }

  function midiDecimal(control) {
    return String(Math.round(fraction(control) * 127));
  }

  // The authored pan label-and-condition pair; a centered pan is a presented
  // value ("C"), never an absence.
  function panCondition(control) {
    var f = fraction(control);
    if (f < 0.45) {
      return "L";
    }
    return f > 0.55 ? "R" : "C";
  }

  function trackName(trackId) {
    var hex = Number(trackId).toString(16).toUpperCase();
    return "T" + (hex.length < 2 ? "0" + hex : hex);
  }

  function surfaceById(model, id) {
    var surfaces = model.surfaces || [];
    for (var i = 0; i < surfaces.length; i += 1) {
      if (surfaces[i].id === id) {
        return surfaces[i];
      }
    }
    return null;
  }

  // The document's surface holding one declared role, resolved by that role
  // rather than by a context fork: `persistentSide` is the Inspector on MIXER
  // and the Utility region on PATCH, `detail` is the subordinate capability
  // surface present exactly while a detail entry is open.
  function surfaceByRole(model, role) {
    var surfaces = model.surfaces || [];
    for (var i = 0; i < surfaces.length; i += 1) {
      if (surfaces[i].role === role) {
        return surfaces[i];
      }
    }
    return null;
  }

  function persistentSideSurface(model) {
    return surfaceByRole(model, "persistentSide");
  }

  function midiSettingsSurface(model) {
    var surface = surfaceById(model, "midiDeviceSettings");
    return surface && surface.summary && surface.summary.kind === "midiDeviceSettings"
      ? surface
      : null;
  }

  function controllerSettingsSurface(model) {
    var surface = surfaceById(model, "controllerSettings");
    return surface && surface.summary && surface.summary.kind === "controllerSettings"
      ? surface
      : null;
  }

  function saveLoadSettingsSurface(model) {
    return surfaceById(model, "saveLoadSettings");
  }

  function settingsSurface(model) {
    return saveLoadSettingsSurface(model) || controllerSettingsSurface(model) || midiSettingsSurface(model);
  }

  // The serialized identity of one projected control ("patch.engine",
  // "patch.effectSlot.0", ...). Used for structure — which group a row joins,
  // which row heads it — and never painted; the same way the mixer bank
  // groups its columns by `id.trackId`.
  function controlIdOf(control) {
    var id =
      control && control.path && control.path.controlId
        ? control.path.controlId.id
        : "";
    return id !== null && typeof id === "object"
      ? JSON.stringify(id)
      : String(id);
  }

  // One surface's control by identity, or null.
  function controlById(surface, id) {
    var controls = (surface && surface.controls) || [];
    for (var i = 0; i < controls.length; i += 1) {
      if (controlIdOf(controls[i]) === id) {
        return controls[i];
      }
    }
    return null;
  }

  function controlByPath(surface, path) {
    var wanted = JSON.stringify(path || null);
    var controls = (surface && surface.controls) || [];
    for (var i = 0; i < controls.length; i += 1) {
      if (JSON.stringify(controls[i].path || null) === wanted) {
        return controls[i];
      }
    }
    return null;
  }

  // Groups the flat mixer-main control list into track columns keyed by
  // track id, preserving the document's declared first-appearance order.
  function trackColumns(mainSurface) {
    var order = [];
    var byTrack = {};
    var controls = (mainSurface && mainSurface.controls) || [];
    for (var i = 0; i < controls.length; i += 1) {
      var control = controls[i];
      var id = control.path.controlId.id;
      if (!id || id.kind !== "track") {
        continue;
      }
      if (!Object.prototype.hasOwnProperty.call(byTrack, id.trackId)) {
        byTrack[id.trackId] = { trackId: id.trackId };
        order.push(byTrack[id.trackId]);
      }
      byTrack[id.trackId][id.parameter] = control;
    }
    return order;
  }

  function columnFocused(column) {
    return columnFocusedControl(column) !== null;
  }

  function columnFocusedControl(column) {
    var parameters = ["level", "pan", "mute", "solo"];
    for (var i = 0; i < parameters.length; i += 1) {
      var control = column[parameters[i]];
      if (control && control.focused) {
        return control;
      }
    }
    return null;
  }

  function columnFocusedRow(column) {
    var parameters = ["level", "pan", "mute", "solo"];
    for (var i = 0; i < parameters.length; i += 1) {
      var parameter = parameters[i];
      if (column[parameter] && column[parameter].focused) {
        return parameter;
      }
    }
    return null;
  }

  function mixerRowState(control, mode) {
    return control ? controlState(control, mode).name : "unavailable";
  }

  // Fill/emphasis state for the column's fader, resolved in declared
  // priority: focus, then error, then disabled, then mute, then solo, then
  // rest. Each is also carried by text or shape elsewhere in the column, so
  // color is never the only signal.
  function faderState(column) {
    if (columnFocused(column)) {
      return "focused";
    }
    var level = column.level;
    if (level && level.error) {
      return "error";
    }
    if (level && level.enabled === false) {
      return "disabled";
    }
    if (column.mute && toggleOn(column.mute)) {
      return "muted";
    }
    if (column.solo && toggleOn(column.solo)) {
      return "soloed";
    }
    return "resting";
  }

  // The focused control anywhere in the document, in surface order.
  function focusedControl(model) {
    var surfaces = model.surfaces || [];
    for (var s = 0; s < surfaces.length; s += 1) {
      var controls = surfaces[s].controls || [];
      for (var c = 0; c < controls.length; c += 1) {
        if (controls[c].focused) {
          return controls[c];
        }
      }
    }
    return null;
  }

  // Cursor identity, e.g. "T00 / LEVEL", derived from the focused control's
  // own label ("T00 Level"); falls back to the focus path's identity parts.
  function focusIdentity(model) {
    var control = focusedControl(model);
    if (control && control.label) {
      var label = String(control.label);
      var match = label.match(/^(T[0-9A-Fa-f]{2})\s+(.+)$/);
      if (match) {
        return match[1].toUpperCase() + " / " + match[2].toUpperCase();
      }
      return label.toUpperCase();
    }
    var id =
      model.focusPath && model.focusPath.controlId && model.focusPath.controlId.id;
    if (id && id.kind === "track") {
      return trackName(id.trackId) + " / " + String(id.parameter).toUpperCase();
    }
    return "";
  }

  // The focused mixer track id, or null — the meter rule and the Inspector
  // both key on it.
  function focusedTrackId(model) {
    var id =
      model.focusPath && model.focusPath.controlId && model.focusPath.controlId.id;
    if (id && (id.kind === "track" || id.kind === "send")) {
      return id.trackId;
    }
    var inspector = surfaceById(model, "mixerInspector");
    var summary = (inspector && inspector.summary) || null;
    return summary && summary.kind === "mixerInspector"
      ? summary.focusedTrack
      : null;
  }

  function statusToneClass(status) {
    if (!status) {
      return "muted";
    }
    if (status.kind === "ready") {
      return "positive";
    }
    return status.kind === "failed" ||
      status.kind === "error" ||
      status.kind === "unavailable"
      ? "warning"
      : "adjust";
  }

  // Condenses a valid action's label to the authored hint form:
  // "Move right" -> "right", "Open PATCH" -> "patch", "Adjust mode" -> "adjust".
  function hintLabel(action) {
    return String(action.label)
      .toLowerCase()
      .replace(/^(open|move)\s+/, "")
      .replace(/\s+mode$/, "");
  }

  // One projected action list as its hint run.
  //
  // The hints are separated by a text space, not by the flex gap alone. Every
  // container that holds a run is a flex container, so a whitespace-only text
  // node between two items generates no anonymous flex item and the painted
  // geometry is unchanged — but the run's `textContent` reads "2:patch 1:mixer"
  // rather than "2:patch1:mixer", which is what a screen reader announces, what
  // a selection copies, and what an observation can compare against the
  // projected list. Adjacent spans with no separating text made the run a
  // single unbroken word in every one of those readings.
  function hintRun(actions) {
    var overrides = hostView ? hostView.actionOverrides || [] : [];
    actions = actions.filter(function (action) {
      return !overrides.some(function (override) {
        return JSON.stringify(override.action) === JSON.stringify(action.action);
      });
    }).concat(overrides);
    var spans = [];
    for (var i = 0; i < actions.length; i += 1) {
      var action = actions[i];
      if (!action.hint) {
        continue; // null hints never render (spike defect, kept fixed)
      }
      spans.push(
        '<span class="type-hint focus action-hint" data-role="action-hint">' +
          escapeHtml(action.hint) +
          ":" +
          escapeHtml(hintLabel(action)) +
          "</span>"
      );
    }
    return spans.join(" ");
  }

  // ---- the shared control-state vocabulary --------------------------------

  // Derives the declared ComponentState treatment for one projected control
  // from the immutable document plus the projected interaction mode —
  // exactly the precedence the pre-cutover shipped row applied: a failed edit
  // outranks an in-flight one, while focus is layered independently below so
  // neither product condition can erase it. Matching is exhaustive over what
  // the document can carry; an unknown lifecycle kind or interaction mode is
  // an explicit visible `?state` marker, never a silent resting row.
  function controlState(control, mode) {
    if (control.error) {
      return { name: "error", raw: null };
    }
    var status = control.status || null;
    if (status) {
      var kind = String(status.kind);
      if (
        kind === "loading" ||
        kind === "validating" ||
        kind === "preparing" ||
        kind === "activating"
      ) {
        return { name: "loading", raw: null };
      }
      if (kind === "unavailable") {
        return { name: "unavailable", raw: null };
      }
      if (kind !== "ready" && kind !== "failed") {
        return { name: "unknown", raw: kind };
      }
    }
    if (control.focused) {
      var modeName = String(mode);
      if (modeName === "adjust") {
        return { name: "adjusting", raw: null };
      }
      if (
        modeName === "navigate" ||
        modeName === "modal" ||
        modeName === "multiSelect"
      ) {
        return { name: "focused", raw: null };
      }
      return { name: "unknown", raw: modeName };
    }
    if (control.enabled && (control.editable || browsableAsset(control))) {
      return { name: "resting", raw: null };
    }
    return { name: "disabled", raw: null };
  }

  // Focus and lifecycle are independent projected facts. A structural row
  // can remain the singular semantic focus while it reports Loading,
  // Unavailable, or Error; collapsing both into `data-state` made the
  // lifecycle treatment hide the focus halo. Keep the state vocabulary for
  // the row's product condition and layer the reducer-owned focus treatment
  // beside it.
  function focusTreatment(control, mode) {
    if (!control || control.focused !== true) {
      return null;
    }
    return String(mode) === "adjust" ? "adjusting" : "focused";
  }

  // Renders one typed document value as finished text. Numeric presentation is
  // driven by the row's projected range, edit step, and unit: milliseconds and
  // physical units stay in their native scale, while authored unitless
  // normalized/bipolar ranges become percentages. Arbitrary fixed precision is
  // never part of the display contract.
  function controlValueText(control) {
    var value = control && control.value;
    if (!value || typeof value !== "object") {
      return UNAVAILABLE_MARK;
    }
    if (value.kind === "scalar") {
      // A `stepped` control is a bounded integer — the MIDI channel and the
      // voice limit, the kind this mission's reducer newly produces. It rides
      // the same scalar envelope a continuous value does, so the row's own
      // declared kind is what tells them apart; without this a channel reads
      // "0.000" and a voice limit "64.000".
      return control.kind === "stepped"
        ? String(Math.round(Number(value.value)))
        : numericValueText(control, Number(value.value));
    }
    if (value.kind === "parameter") {
      var parameter = value.value;
      if (parameter && typeof parameter === "object") {
        if (parameter.kind === "continuous") {
          return String(control.selectedLabel ?? numericValueText(control, Number(parameter.value)));
        }
        if (parameter.kind === "stepped") {
          return String(control.selectedLabel ?? parameter.value);
        }
        if (parameter.kind === "choice") {
          // The document's own authored name for the stored id, never a name
          // composed here. `parameter.value` is the choice *identity* —
          // `sf2.bank-0.program-40`, `braids.model.csaw` — and painting it is
          // the defect FR-014 closes; DESIGN.md calls this exact row "the
          // authored-name Preset row". The projection carries the descriptor's
          // label beside the id as `selectedLabel`, so the page reads a name it
          // was given (mission finding F-33). A choice with no projected label
          // falls back to the id rather than to a blank, because an unlabelled
          // option is a descriptor defect and hiding it would hide that.
          return String(
            control.selectedLabel === null || control.selectedLabel === undefined
              ? parameter.value
              : control.selectedLabel
          );
        }
        if (parameter.kind === "toggle") {
          return parameter.value ? "ON" : "OFF";
        }
        return "?" + String(parameter.kind);
      }
      return UNAVAILABLE_MARK;
    }
    if (value.kind === "asset") {
      return value.value && value.value.locator
        ? String(value.value.locator)
        : UNAVAILABLE_MARK;
    }
    if (value.kind === "identity" || value.kind === "summary") {
      return String(value.value);
    }
    return "?" + String(value.kind);
  }

  function normalizedPercentage(control) {
    var range = control && control.numericRange;
    if (control && control.unit) {
      return false;
    }
    if (
      !range ||
      control.selectedLabel != null ||
      typeof range.minimum !== "number" ||
      typeof range.maximum !== "number"
    ) {
      return false;
    }
    return (
      (range.minimum === 0 && range.maximum === 1) ||
      (range.minimum === -1 && range.maximum === 1)
    );
  }

  function decimalPlacesForStep(step) {
    if (!Number.isFinite(step) || step <= 0) {
      return 3;
    }
    for (var places = 0; places <= 6; places += 1) {
      var scaled = step * Math.pow(10, places);
      if (Math.abs(scaled - Math.round(scaled)) < 0.0000001) {
        return places;
      }
    }
    return 6;
  }

  function compactNumericText(value, step) {
    if (!Number.isFinite(value)) {
      return UNAVAILABLE_MARK;
    }
    var places = Math.max(3, decimalPlacesForStep(step));
    var text = value
      .toFixed(places)
      .replace(/\.0+$/, "")
      .replace(/(\.\d*?[1-9])0+$/, "$1");
    return text === "-0" ? "0" : text;
  }

  function numericValueText(control, value) {
    var percentage = normalizedPercentage(control);
    var scale = percentage ? 100 : 1;
    var range = control && control.numericRange;
    var step =
      range && typeof range.fineStep === "number"
        ? range.fineStep * scale
        : 0.001 * scale;
    return compactNumericText(value * scale, step);
  }

  function controlUnitText(control) {
    if (control && control.selectedLabel != null) {
      return "";
    }
    if (control && control.unit) {
      return String(control.unit);
    }
    return normalizedPercentage(control) ? "%" : "";
  }

  // The non-color signal a state carries beside its row (state.rs
  // NonColorSignal): Disabled says "Locked", Loading says the document's own
  // lifecycle word, Error says the typed failure text, an unknown state says
  // its explicit `?state` marker. Shape-signalled states return no text.
  function stateMarkHtml(control, state) {
    if (state.name === "disabled") {
      return '<span class="muted">Locked</span>';
    }
    if (state.name === "loading") {
      var word = control.status ? String(control.status.label) : "";
      return '<span class="adjust">' + escapeHtml(word) + "</span>";
    }
    if (state.name === "unavailable") {
      return '<span class="warning">Unavailable</span>';
    }
    if (state.name === "error") {
      var failure = control.error ? String(control.error.label) : "";
      return '<span class="warning">' + escapeHtml(failure) + "</span>";
    }
    if (state.name === "unknown") {
      return '<span class="warning">?' + escapeHtml(String(state.raw)) + "</span>";
    }
    return "";
  }

  // ---- band renderers (pure: document in, HTML string out) ---------------

  function contextLineHtml(model) {
    var settings = settingsSurface(model);
    if (settings) {
      return (
        '<span class="type-heading">CREST SYNTH</span>' +
        '<span class="spring"></span>' +
        '<span class="type-label focus context-entry" data-context="settings" data-active="true"><span class="patch">*</span> SETTINGS</span>' +
        (settings.id === "midiDeviceSettings"
          ? '<span class="type-label positive" data-role="status">WATCHING</span>'
          : "")
      );
    }
    // The two declared top-level contexts; the active one carries the
    // authored "*" marker so activity is legible beyond color.
    var contexts = ["patch", "mixer"];
    var run = "";
    for (var i = 0; i < contexts.length; i += 1) {
      var name = contexts[i];
      var active = model.context === name;
      run +=
        '<span class="type-label context-entry' +
        (active ? " focus" : " muted") +
        '" data-context="' +
        name +
        '"' +
        (active ? ' data-active="true"' : "") +
        ">" +
        (active ? '<span class="patch">*</span> ' : "") +
        name.toUpperCase() +
        "</span>";
    }
    var status = model.status || null;
    var statusHtml = status
      ? '<span class="type-label ' +
        statusToneClass(status) +
        '" data-role="status">' +
        escapeHtml(status.label) +
        "</span>"
      : "";
    var focusRepair = model.focusRepair || null;
    var repairHtml = focusRepair
      ? '<span class="type-hint warning focus-repair" data-role="focus-repair">' +
        escapeHtml(String(focusRepair.label)) +
        "</span>"
      : "";
    return (
      '<span class="type-heading">CREST SYNTH</span>' +
      '<span class="spring"></span>' +
      run +
      repairHtml +
      statusHtml
    );
  }

  // The identity band, one shared skeleton for both contexts: the context
  // display, a projected metadata run, and the focus annotation. Only the
  // metadata content differs, and it comes from the document's own surface
  // summaries, never from a page-side fork.
  function identityHeaderHtml(model, columns) {
    var settings = settingsSurface(model);
    if (settings) {
      return (
        '<span class="type-heading">' +
        escapeHtml(String(settings.summary.title || settings.label || "SETTINGS").toUpperCase()) +
        '</span><span class="type-label muted">/ ' +
        (settings.id === "saveLoadSettings" ? "SESSION FILES" : "PHYSICAL INPUT") + '</span>' +
        '<span class="spring"></span>' +
        '<span class="type-value focus" data-role="focus-annotation">' +
        escapeHtml(focusIdentity(model)) +
        "</span>"
      );
    }
    var sendSurface = surfaceById(model, "sends");
    if (sendSurface) {
      return '<span class="type-display">SENDS</span><span class="type-label muted">/ ' + String(sendSurface.summary.count) + ' CHAINS</span><span class="spring"></span><span class="type-value focus">' + escapeHtml(focusIdentity(model)) + '</span>';
    }
    var metadata;
    if (model.context === "patch") {
      var main = surfaceById(model, "patchMain");
      var summary = (main && main.summary) || null;
      // The Patch's projected name and the engine's projected *label* — the
      // engine row's own value, which is the descriptor's authored label.
      // `summary.capabilityId` used to sit here; it is the capability's
      // identity, not its name, and an identity on screen is exactly the
      // defect FR-014 closes (see cross-WP finding F-28, which counts
      // `activeCapabilityId` a serialization key).
      var engine = controlById(main, "patch.engine");
      if (hasWaveform(surfaceByRole(model, "detail")) && !surfaceByRole(model, "modal")) {
        return '<span class="type-label patch" data-role="sample-patch-number">' +
          escapeHtml(summary && summary.kind === "patch"
            ? "PATCH " + String(summary.patchId).padStart(2, "0") : "PATCH " + String(summary ? summary.activeCount : 0).padStart(2, "0")) +
          '</span><span class="type-heading" data-role="sample-patch-name">/ ' +
          escapeHtml(summary && summary.kind === "patch" ? String(summary.patchName) : "DEFAULT") +
          '</span><span class="spring"></span><span class="type-hint muted" data-role="sample-subject-annotation">' +
          escapeHtml(engine ? controlValueText(engine) : UNAVAILABLE_MARK) +
          ' · INSTRUMENT DETAIL</span>';
      }
      metadata = summary && summary.kind === "patch"
        ? escapeHtml(String(summary.patchName)) +
          HINT_SEPARATOR +
          escapeHtml(engine ? controlValueText(engine) : UNAVAILABLE_MARK)
        : summary && summary.kind === "emptyPatch"
          ? "PATCH " + String(summary.activeCount).padStart(2, "0") +
            HINT_SEPARATOR +
            "DEFAULT " +
            escapeHtml(engine ? controlValueText(engine) : UNAVAILABLE_MARK) +
            HINT_SEPARATOR +
            "CAPACITY " +
            escapeHtml(String(summary.activeCount)) +
            "/" +
            escapeHtml(String(summary.capacity))
          : UNAVAILABLE_MARK;
    } else {
      metadata = columns.length + " TRACKS";
    }
    return (
      '<span class="type-display">' +
      escapeHtml(String(model.context || "").toUpperCase()) +
      "</span>" +
      '<span class="type-label muted">/ ' +
      metadata +
      "</span>" +
      '<span class="spring"></span>' +
      '<span class="type-value focus" data-role="focus-annotation">' +
      escapeHtml(focusIdentity(model)) +
      "</span>"
    );
  }

  function columnHtml(column, model) {
    var focusedControl = columnFocusedControl(column);
    var focused = focusedControl !== null;
    var correlated = focusedTrackId(model) === column.trackId;
    var focusedRow = columnFocusedRow(column);
    var focusPathAttribute = focusedControl
      ? ' data-focus-path="' +
        escapeHtml(JSON.stringify(focusedControl.path || null)) +
        '"'
      : "";
    var mode = model.interactionMode;
    var state = faderState(column);
    var header =
      '<span class="structure track-header type-label ' +
      (focused ? "focus" : "secondary") +
      '" data-structure="TrackHeader">' +
      trackName(column.trackId) +
      "</span>";

    var fader;
    if (column.level) {
      var level = fraction(column.level).toFixed(6);
      fader =
        '<div class="structure level-fader" data-structure="LevelFader"' +
        ' data-control-row="level" data-row-state="' +
        mixerRowState(column.level, mode) +
        '" data-state="' +
        state +
        '" data-level="' +
        level +
        '">' +
        '<div class="fader-track"><div class="fader-fill"></div>' +
        '<div class="fader-cap"></div></div>' +
        '<div class="track-meter" data-meter-track="' +
        column.trackId +
        '" data-meter-state="stale" aria-label="' +
        trackName(column.trackId) +
        ' meter stale"><div class="track-meter-fill"></div><div class="track-meter-peak"></div></div></div>';
    } else {
      fader =
        '<div class="structure level-fader unavailable" data-structure="LevelFader"' +
        ' data-control-row="level" data-row-state="unavailable"' +
        ' data-state="unavailable"><span class="type-hint muted">' +
        UNAVAILABLE +
        '</span><div class="track-meter" data-meter-track="' +
        column.trackId +
        '" data-meter-state="stale" aria-label="' +
        trackName(column.trackId) +
        ' meter stale"><div class="track-meter-fill"></div><div class="track-meter-peak"></div></div></div>';
    }

    var readout = column.level
      ? '<span class="structure level-readout type-value ' +
        (column.level.focused ? "focus" : "patch") +
        '" data-structure="LevelReadout">' +
        midiHex(column.level) +
        "</span>"
      : '<span class="structure level-readout type-value muted unavailable"' +
        ' data-structure="LevelReadout">' +
        UNAVAILABLE +
        "</span>";

    var pan = column.pan
      ? '<span class="structure pan-readout type-hint" data-structure="PanReadout"' +
        ' data-control-row="pan" data-row-state="' +
        mixerRowState(column.pan, mode) +
        '">' +
        '<span class="muted">P</span> <span class="secondary">' +
        panCondition(column.pan) +
        "</span></span>"
      : '<span class="structure pan-readout type-hint muted unavailable"' +
        ' data-structure="PanReadout">P ' +
        UNAVAILABLE +
        "</span>";

    // Mute and solo together on one line, resting condition included, so an
    // engaged pair is never reduced to whichever was set last and a resting
    // line never reads as an unavailable one.
    var stateLine;
    if (column.mute || column.solo) {
      var mutePart = column.mute
        ? toggleOn(column.mute)
          ? '<span class="state-part warning" data-control-row="mute" data-row-state="' +
            mixerRowState(column.mute, mode) +
            '">M ON</span>'
          : '<span class="state-part muted" data-control-row="mute" data-row-state="' +
            mixerRowState(column.mute, mode) +
            '">M --</span>'
        : '<span class="muted">M ' + UNAVAILABLE + "</span>";
      var soloPart = column.solo
        ? toggleOn(column.solo)
          ? '<span class="state-part positive" data-control-row="solo" data-row-state="' +
            mixerRowState(column.solo, mode) +
            '">S ON</span>'
          : '<span class="state-part muted" data-control-row="solo" data-row-state="' +
            mixerRowState(column.solo, mode) +
            '">S --</span>'
        : '<span class="muted">S ' + UNAVAILABLE + "</span>";
      // The separator carries no surrounding spaces so the state marks retain
      // their intrinsic single-line form as the bounded column track reflows.
      stateLine =
        '<span class="structure state-line type-hint" data-structure="StateLine">' +
        mutePart +
        '<span class="muted">·</span>' +
        soloPart +
        "</span>";
    } else {
      stateLine =
        '<span class="structure state-line type-hint muted unavailable"' +
        ' data-structure="StateLine">M/S ' +
        UNAVAILABLE +
        "</span>";
    }

    return (
      '<div class="column' +
      (focused ? " focused" : correlated ? " correlated" : "") +
      '" data-track="' +
      column.trackId +
      '" data-focused-row="' +
      (focusedRow === null ? "" : focusedRow) +
      '"' +
      focusPathAttribute +
      ">" +
      header +
      fader +
      readout +
      pan +
      stateLine +
      "</div>"
    );
  }

  // The meter element every workspace carries: render always writes the
  // zero/stale state (or nothing when no mixer track is focused) and only
  // the meter listener may overwrite it.
  function meterHtml(model) {
    if (focusedTrackId(model) === null) return "";
    return '<span class="master-meter" data-role="master-meter" data-meter-state="stale">' +
      '<span class="type-hint secondary" id="meter-readout">MASTER — dBFS</span>' +
      '<span class="master-meter-rail"><span class="track-meter-fill"></span><span class="track-meter-peak"></span></span>' +
      '<span class="type-hint" id="master-clip">0 dBFS</span></span>';
  }

  // The shared workspace scaffold both contexts ride: one caption row
  // (context-specific left run, the meter, a spring, and a context-specific
  // right run) followed by the workspace body. Current action guidance has
  // one visual owner, the persistent shell footer; repeating subsets here
  // makes the workspace compete with the controls it is meant to explain.
  function workspaceScaffold(model, leftHtml, rightHtml, bodyHtml) {
    return (
      '<div class="caption-row">' +
      leftHtml +
      (!model || surfaceById(model, "sends") ? "" : meterHtml(model)) +
      '<span class="spring"></span>' +
      rightHtml +
      "</div>" +
      bodyHtml
    );
  }

  function mixerWorkspaceHtml(model, columns) {
    var bank;
    if (columns.length > 0) {
      var cells = "";
      for (var i = 0; i < columns.length; i += 1) {
        cells += columnHtml(columns[i], model);
      }
      bank = '<div class="bank" id="bank">' + cells + "</div>";
    } else {
      // A bank with no track view data marks the bank unavailable rather
      // than painting a representative mixer (MixerStripBank rule).
      bank =
        '<div class="bank bank-unavailable" id="bank">' +
        '<span class="type-label muted">MIXER BANK ' +
        UNAVAILABLE +
        "</span></div>";
    }
    return workspaceScaffold(
      model,
      '<span class="type-label muted">LEVEL / PAN / MUTE / SOLO</span>',
      "",
      bank
    );
  }

  function midiMarkerGlyph(marker) {
    return {
      openCircle: "○",
      progressRing: "◌",
      filledCircle: "●",
      slashedCircle: "⊘",
      stopSquare: "■",
      errorDiamond: "◆",
    }[marker] || "◇";
  }

  function midiFact(value) {
    return value === null || value === undefined || value === ""
      ? "UNKNOWN / NOT REPORTED"
      : String(value).toUpperCase();
  }

  // Page selection is reducer-owned. These labels advertise sibling system
  // pages without creating another keyboard or semantic focus target.
  function settingsPagesHtml(surface) {
    var pages = [
      { id: "midiDeviceSettings", label: "MIDI DEVICES" },
      { id: "controllerSettings", label: "CONTROLLER BUTTONS" },
      { id: "saveLoadSettings", label: "SAVE & LOAD" },
    ];
    var body = "";
    for (var i = 0; i < pages.length; i += 1) {
      var current = surface && surface.id === pages[i].id;
      body += '<span class="type-label settings-page ' +
        (current ? "secondary" : "muted") +
        '" data-settings-page="' + escapeHtml(pages[i].id) + '"' +
        (current ? ' aria-current="page"' : "") + '>' +
        (current ? '● ' : '') + escapeHtml(pages[i].label) + '</span>';
    }
    return '<div class="settings-pages" aria-label="Settings pages">' + body + '</div>';
  }

  function saveLoadSettingsWorkspaceHtml(model) {
    var surface = saveLoadSettingsSurface(model);
    var controls = (surface && surface.controls) || [];
    var busy = !latestSessionDocument || latestSessionDocument.marker === "busy";
    var body = "";
    for (var i = 0; i < controls.length; i += 1) {
      var control = controls[i];
      var focused = Boolean(control.focused);
      body += '<div class="midi-device-row session-file-row' + (focused ? ' focused' : '') +
        '" data-focus-path="' + escapeHtml(JSON.stringify(control.path)) + '"' +
        ' aria-disabled="' + String(busy) + '"' +
        (focused ? ' data-focus-treatment="focused"' : '') + '>' +
        '<span class="midi-status-marker" aria-hidden="true">' + (focused ? '›' : '·') + '</span>' +
        '<div class="midi-row-identity"><span class="type-label ' + (focused ? 'focus' : 'secondary') + '">' +
        escapeHtml(control.label) + '</span><span class="type-hint secondary">' +
        escapeHtml(controlValueText(control)) + '</span></div>' +
        '<span class="type-hint muted session-action-state">' + (busy ? 'WAIT' : 'READY') + '</span></div>';
    }
    return settingsPagesHtml(surface) + workspaceScaffold(model,
      '<span class="type-label muted">SESSION FILES</span>',
      '<span class="type-hint secondary">.CREST</span>',
      '<p class="type-hint secondary session-file-description">' + escapeHtml(surface.summary.description) + '</p>' +
      '<div class="midi-device-list" data-role="session-file-list">' + body + '</div>');
  }

  function saveLoadSettingsInspectorHtml() {
    var document = latestSessionDocument;
    if (!document) {
      return '<div class="inspector-pinned"><span class="type-label muted">CURRENT SESSION</span>' +
        '<span class="type-value secondary">Waiting for document status</span></div>';
    }
    var marker = document.marker === "error" ? '! ' : document.marker === "busy" ? '… ' : '✓ ';
    return '<div class="inspector-pinned session-document" data-document-marker="' + escapeHtml(document.marker) + '">' +
      '<span class="type-label muted">CURRENT SESSION</span>' +
      '<span class="type-heading session-document-name" data-role="session-name">' + escapeHtml(document.name) + '</span>' +
      '<span class="type-label ' + (document.dirty ? 'warning' : 'secondary') + '" data-role="session-dirty">' +
      (document.dirty ? '● UNSAVED CHANGES' : '✓ NO UNSAVED CHANGES') + '</span></div>' +
      '<div class="session-document-status" role="status" aria-live="polite">' +
      '<span class="type-label ' + (document.marker === "error" ? 'warning' : 'secondary') + '" data-role="session-status">' +
      escapeHtml(marker + (document.operation ? document.operation + ' · ' : '') + document.status) + '</span>' +
      (document.failure ? '<span class="type-hint warning" data-role="session-failure">' + escapeHtml(document.failure) + '</span>' : '') +
      '<span class="type-hint muted">Session files reference assets in your libraries; they do not include audio files. MIDI devices and controller buttons are separate preferences.</span></div>';
  }

  // Read-only shell facts; no document paths, lifecycle decisions or synth
  // state live in the page. Updates also arrive when product generation stays put.
  function observeSessionDocument(document) {
    latestSessionDocument = document;
    if (latestModel && saveLoadSettingsSurface(latestModel)) {
      var doc = window.document;
      doc.getElementById("workspace").innerHTML = saveLoadSettingsWorkspaceHtml(latestModel);
      doc.getElementById("inspector").innerHTML = saveLoadSettingsInspectorHtml();
      revealSemanticFocus(doc, latestModel);
    }
  }

  function midiSettingsWorkspaceHtml(model) {
    var surface = midiSettingsSurface(model);
    var summary = surface ? surface.summary : null;
    var rows = (summary && summary.rows) || [];
    var controls = (surface && surface.controls) || [];
    var body = "";
    if (rows.length === 0) {
      var root = controls.length > 0 ? controls[0] : null;
      body =
        '<div class="midi-empty" data-focus-path="' +
        escapeHtml(JSON.stringify((root && root.path) || null)) +
        '"><span class="midi-status-marker" data-marker="openCircle">○</span>' +
        '<div><span class="type-label">NO MIDI INPUTS FOUND</span>' +
        '<span class="type-hint muted">WATCHING FOR DEVICES</span></div></div>';
    } else {
      for (var i = 0; i < rows.length; i += 1) {
        var row = rows[i];
        var control = controls[i] || null;
        var facts = (row.descriptor && row.descriptor.portFacts) || null;
        var focused = Boolean(row.focused);
        body +=
          '<div class="midi-device-row' +
          (focused ? " focused" : "") +
          '" data-midi-status="' +
          escapeHtml(String(row.statusText || "")) +
          '" data-marker="' +
          escapeHtml(String(row.marker || "")) +
          '" data-focus-path="' +
          escapeHtml(JSON.stringify((control && control.path) || null)) +
          '">' +
          '<span class="midi-status-marker" aria-label="' +
          escapeHtml(String(row.statusText || "UNKNOWN")) +
          '">' +
          midiMarkerGlyph(row.marker) +
          "</span>" +
          '<div class="midi-row-identity"><span class="type-value ' +
          (focused ? "focus" : "secondary") +
          '">' +
          escapeHtml(String(row.descriptor.displayName || UNAVAILABLE)) +
          '</span><span class="type-hint muted">' +
          escapeHtml(midiFact(facts && facts.manufacturer)) +
          HINT_SEPARATOR +
          escapeHtml(midiFact(facts && facts.transport)) +
          "</span></div>" +
          '<span class="midi-row-status type-label">' +
          escapeHtml(String(row.statusText || "UNKNOWN")) +
          "</span>" +
          (row.action
            ? '<span class="type-hint secondary midi-row-action">' +
              escapeHtml(String(row.action).toUpperCase()) +
              "</span>"
            : "") +
          "</div>";
      }
    }
    return settingsPagesHtml(surface) + workspaceScaffold(
      model,
      '<span class="type-label muted">AVAILABLE INPUTS</span>',
      '<span class="type-hint positive">' +
        escapeHtml(String((summary && summary.summary) || "WATCHING")) +
        "</span>",
      '<div class="midi-device-list" data-role="midi-device-list">' + body + "</div>"
    );
  }

  function controllerSettingsWorkspaceHtml(model) {
    var surface = controllerSettingsSurface(model);
    var controls = (surface && surface.controls) || [];
    var body = "";
    for (var i = 0; i < controls.length; i += 1) {
      var control = controls[i];
      var focused = Boolean(control.focused);
      body += '<div class="midi-device-row controller-button-row' +
        (focused ? " focused" : "") +
        '" data-editable="' + String(Boolean(control.editable)) +
        '" data-focus-path="' + escapeHtml(JSON.stringify(control.path || null)) + '"' +
        (focused ? ' data-focus-treatment="focused"' : '') + '>' +
        '<span class="midi-status-marker" aria-hidden="true">' + (focused ? '›' : '·') + '</span>' +
        '<div class="midi-row-identity"><span class="type-label ' +
        (focused ? "focus" : "secondary") + '">' + escapeHtml(control.label) + '</span></div>' +
        '<span class="type-value controller-button-value">' +
        escapeHtml(controlValueText(control)) + '</span></div>';
    }
    return settingsPagesHtml(surface) + workspaceScaffold(
      model,
      '<span class="type-label muted">BUTTON ASSIGNMENTS</span>',
      '<span class="type-hint secondary">SHARED MAPPING</span>',
      '<div class="midi-device-list" data-role="controller-button-list">' + body + '</div>'
    );
  }

  function controllerSettingsInspectorHtml(model) {
    var surface = controllerSettingsSurface(model);
    var summary = (surface && surface.summary) || {};
    var controller = summary.controller || {};
    var devices = controller.devices || [];
    var deviceNames = "";
    for (var i = 0; i < devices.length; i += 1) {
      deviceNames += '<span class="type-value secondary">' +
        escapeHtml(devices[i].name) + '</span>';
    }
    var status = controller.preferenceStatus || {};
    var tone = controller.backendFailure || status.kind === "failed"
      ? "warning" : controller.capture ? "adjust" : "secondary";
    return '<div class="inspector-pinned"><span class="type-label muted">CONNECTED CONTROLLERS</span>' +
      (deviceNames || '<span class="type-heading secondary">NONE</span>') +
      '<span class="type-hint muted">' +
      (devices.length ? 'One mapping applies to all connected gamepads.' : 'No controllers connected. Connect a gamepad to assign buttons.') +
      '</span></div>' +
      '<div class="controller-settings-status" data-controller-capture="' +
      escapeHtml(controller.capture || "") + '" data-controller-preference-status="' +
      escapeHtml(status.kind || "") + '"><span class="type-label ' + tone + '">' +
      escapeHtml(summary.summary || UNAVAILABLE) + '</span>' +
      '<span class="type-value focus">' + escapeHtml(focusIdentity(model)) + '</span>' +
      '<span class="type-hint secondary">' + escapeHtml(summary.description || "") + '</span>' +
      '<span class="type-hint muted">Assigning a button already in use swaps the two assignments.</span></div>';
  }

  function midiInputInspectorHtml(model) {
    var surface = midiSettingsSurface(model);
    var summary = surface ? surface.summary : null;
    var inspector = summary && summary.activeInspector;
    if (!inspector) {
      return (
        '<div class="inspector-pinned"><span class="type-label muted">ACTIVE INPUT</span>' +
        '<span class="type-display secondary">NONE</span></div>' +
        '<span class="type-hint muted">Connect one available input to inspect its acknowledged identity and live activity.</span>'
      );
    }
    var descriptor = inspector.descriptor || {};
    var facts = descriptor.portFacts || {};
    return (
      '<div class="inspector-pinned" data-midi-revision="' +
      escapeHtml(String(inspector.revision)) +
      '"><span class="type-label muted">ACTIVE INPUT</span>' +
      '<span class="type-heading positive">' +
      escapeHtml(String(descriptor.displayName || UNAVAILABLE)) +
      '</span><span class="type-hint secondary">CONNECTED <span class="midi-inline-marker">●</span></span></div>' +
      '<table class="type-hint secondary midi-facts"><tbody>' +
      '<tr><td>TRANSPORT</td><td>' +
      escapeHtml(midiFact(facts.transport)) +
      '</td></tr><tr><td>MANUFACTURER</td><td>' +
      escapeHtml(midiFact(facts.manufacturer)) +
      '</td></tr><tr><td>PRODUCT</td><td>' +
      escapeHtml(midiFact(facts.product)) +
      '</td></tr><tr><td>ACTIVITY</td><td id="midi-activity-state" data-midi-activity-state="stale">WAITING</td></tr></tbody></table>' +
      '<div class="midi-activity-card" id="midi-activity-card" data-midi-activity-state="stale">' +
      '<span class="type-label muted">LIVE ACTIVITY</span>' +
      '<span class="type-hint secondary">WAITING FOR MATCHING INPUT</span></div>'
    );
  }

  // ---- shared PATCH control rows -----------------------------------------

  // One projected range endpoint in the same unit and precision as its value.
  function rangeEndpointText(control, value) {
    return control.kind === "continuous"
      ? numericValueText(control, Number(value))
      : String(value);
  }

  // The projected bounds of a numeric row, or nothing at all when the
  // document carries none — an absent range is a projected fact, not a gap to
  // fill (FR-013).
  function rangeHtml(control) {
    var range = control && control.numericRange;
    if (
      !range ||
      control.selectedLabel != null ||
      typeof range.minimum !== "number" ||
      typeof range.maximum !== "number"
    ) {
      return "";
    }
    return (
      '<span class="prow-range type-hint muted" data-role="row-range">' +
      escapeHtml(rangeEndpointText(control, range.minimum)) +
      escapeHtml(RANGE_SEPARATOR) +
      escapeHtml(rangeEndpointText(control, range.maximum)) +
      "</span>"
    );
  }

  // Asset assignment uses the admitted related-surface action, independently
  // of scalar editability. Never label an accessible browser row read-only.
  function browsableAsset(control) {
    return control && control.kind === "asset" &&
      (control.validActions || []).some(function (entry) {
        return entry.action && entry.action.kind === "openRelated";
      });
  }

  function readOnly(control) {
    return control && control.patchInteraction === "readOnly" && !browsableAsset(control);
  }

  // One projected control as a subordinate or Utility row: label left, the state's
  // non-color mark or the position indicator in the middle, then the rendered
  // value, its unit, its projected bounds, the capability's read-only
  // declaration, and the row's own valid actions — the shipped parameter-row
  // anatomy, widened by the facts FR-011/FR-013 project and nothing painted
  // it. The derived state rides the row as data plus a class so every
  // treatment resolves from the token vocabulary in page.css.
  function patchRowHtml(control, mode, role, ordinal) {
    var state = controlState(control, mode);
    var focus = focusTreatment(control, mode);
    var id = controlIdOf(control);
    var focusPath = JSON.stringify(control.path || null);
    var semanticId =
      control && control.path && control.path.controlId
        ? control.path.controlId.id
        : null;
    var mixerAttributes = "";
    if (semanticId && typeof semanticId === "object") {
      mixerAttributes +=
        ' data-mixer-kind="' + escapeHtml(String(semanticId.kind || "")) + '"';
      if (semanticId.trackId !== undefined) {
        mixerAttributes += ' data-mixer-track="' + semanticId.trackId + '"';
      }
      if (semanticId.bus !== undefined) {
        mixerAttributes += ' data-mixer-bus="' + semanticId.bus + '"';
      }
      if (semanticId.parameter !== undefined) {
        mixerAttributes +=
          ' data-mixer-parameter="' +
          escapeHtml(String(semanticId.parameter)) +
          '"';
      }
    }
    var browse = browsableAsset(control);
    var locked = readOnly(control);
    // Use the projected explanation when present; otherwise keep the generic
    // read-only mark in the same slot.
    var mark = locked ? "" : stateMarkHtml(control, state);
    var middle;
    if (mark) {
      middle =
        '<span class="prow-mark type-hint" data-role="state-mark">' +
        mark +
        "</span>" +
        '<span class="spring"></span>';
    } else {
      var position = positionFraction(control);
      middle =
        position === null
          ? '<span class="spring"></span>'
          : '<div class="prow-position"><div class="prow-position-fill"' +
            ' data-position="' +
            position.toFixed(6) +
            '"></div></div>';
    }
    var unitText = controlUnitText(control);
    var unit = unitText
      ? '<span class="prow-unit type-hint muted">' +
        escapeHtml(unitText) +
        "</span>"
      : "";
    var lockedMark = locked
      ? '<span class="prow-readonly type-hint muted" data-role="read-only">' +
        escapeHtml(control.readOnlyLabel || READ_ONLY_MARK) +
        "</span>"
      : browse
        ? '<span class="type-hint" data-role="browse">BROWSE</span>'
        : "";
    var indexed = ordinal !== null && ordinal !== undefined;
    var ordinalMarkup = indexed
      ? '<span class="prow-cursor type-label" aria-hidden="true">' +
        (focus ? "&gt;" : "&nbsp;") +
        '</span><span class="prow-index type-label secondary">' +
        String(ordinal).padStart(2, "0") +
        "</span>"
      : "";
    var row =
      '<div class="prow' +
      (role === "panel" ? " panel" : "") +
      '" data-control="' +
      escapeHtml(id) +
      '" data-focus-path="' +
      escapeHtml(focusPath) +
      '" data-state="' +
      state.name +
      '" data-focus-treatment="' +
      (focus || "none") +
      '" data-enabled="' +
      String(control.enabled === true) +
      '" data-visible="' +
      String(control.visible === true) +
      '" data-editable="' +
      String(control.editable === true) +
      '" data-valid-action-count="' +
      String((control.validActions || []).length) +
      '" data-browsable="' +
      String(browse) +
      '"' +
      mixerAttributes +
      (control.patchInteraction
        ? ' data-interaction="' + escapeHtml(String(control.patchInteraction)) + '"'
        : "") +
      ">" +
      ordinalMarkup +
      '<span class="prow-label type-label">' +
      escapeHtml(String(control.label)) +
      "</span>" +
      middle +
      '<span class="prow-value type-value">' +
      escapeHtml(controlValueText(control)) +
      "</span>" +
      unit +
      rangeHtml(control) +
      lockedMark +
      "</div>";
    return row + lifecycleHtml(control, id);
  }

  // The lifecycle band beneath a row reporting a structural edit (error,
  // in-flight status, or a requested value): the typed failure and/or
  // lifecycle word, the active and requested graph revisions, and the
  // requested value the edit is moving the row toward — the shipped row's
  // readout (patch_strip_row::render_lifecycle) with the one field it could
  // only mark unavailable now filled from the projection. It takes no focus
  // and appears only while the document reports the edit.
  //
  // This is the declared structural-edit vocabulary and stays the only one:
  // the active value never leaves the row above, so active, requested, and
  // status read together and the active graph stays explicit throughout
  // (FR-010).
  function lifecycleHtml(control, id) {
    var status = control.status || null;
    var inFlight =
      status &&
      (status.kind === "loading" ||
        status.kind === "validating" ||
        status.kind === "preparing" ||
        status.kind === "activating");
    var requested = control.requestedValue || null;
    if (!control.error && !inFlight && !requested) {
      return "";
    }
    var parts = [];
    if (control.error) {
      parts.push(String(control.error.label));
    }
    if (status) {
      parts.push(String(status.label));
      parts.push("ACTIVE GRAPH");
      parts.push(String(status.graphRevision));
      parts.push("REQUESTED GRAPH");
      parts.push(
        status.targetGraphRevision === null ||
          status.targetGraphRevision === undefined
          ? UNAVAILABLE_MARK
          : String(status.targetGraphRevision)
      );
    }
    // Rendered through the same value presentation the active value uses, so
    // the two read on the same terms. A row mid-edit that projects no
    // requested value is still marked rather than filled, so an absent one is
    // never mistaken for the active value under another name.
    parts.push("REQUESTED VALUE");
    parts.push(
      requested
        ? controlValueText({
            kind: control.kind,
            value: requested,
            numericRange: control.numericRange,
            unit: control.unit,
            // The requested value's own authored name, so the band never
            // paints a choice id beneath a row whose active value reads its
            // name (F-33, the requested half).
            selectedLabel: control.requestedLabel,
          })
        : UNAVAILABLE_MARK
    );
    var tone = control.error ? "warning" : "adjust";
    return (
      '<div class="lifecycle type-hint ' +
      tone +
      '" data-lifecycle-for="' +
      escapeHtml(id) +
      '">' +
      escapeHtml(parts.join(HINT_SEPARATOR)) +
      "</div>"
    );
  }

  // A designed structure the projection supplied no entry for: its static
  // name stays on screen and the authored row mark sits where its value
  // would be (section::mark_unavailable).
  function markUnavailableRowHtml(structure) {
    return (
      '<div class="prow unavailable" data-state="disabled">' +
      '<span class="prow-label type-label muted">' +
      escapeHtml(structure) +
      "</span>" +
      '<span class="spring"></span>' +
      '<span class="prow-value type-value muted">' +
      UNAVAILABLE_MARK +
      "</span>" +
      "</div>"
    );
  }

  function overviewHeaderHtml(model, main) {
    var summary = (main && main.summary) || {};
    var empty = summary.kind === "emptyPatch";
    var side = persistentSideSurface(model);
    var routing = ["patch.midiInput", "patch.output.outputTrack"];
    var routeText = "";
    for (var i = 0; i < routing.length; i += 1) {
      var routed = controlById(side, routing[i]);
      if (routed) {
        routeText +=
          '<span class="type-hint muted">' +
          escapeHtml(String(routed.label)) +
          " " +
          escapeHtml(controlValueText(routed)) +
          "</span>";
      }
    }
    return (
      '<header class="overview-heading" data-role="overview-heading">' +
      '<span class="type-hint patch">PATCH ' +
      escapeHtml(empty ? "NEW" : String(summary.patchId || UNAVAILABLE_MARK)) +
      "</span>" +
      '<span class="type-display" data-role="overview-patch-name">' +
      escapeHtml(empty ? String(summary.activeCount).padStart(2, "0") + " / DEFAULT" : String(summary.patchName || UNAVAILABLE_MARK)) +
      "</span>" +
      (empty
        ? '<span class="type-hint ' +
          (summary.creationAvailable ? "adjust" : "warning") +
          '" data-role="patch-capacity">CAPACITY ' +
          escapeHtml(String(summary.activeCount)) +
          "/" +
          escapeHtml(String(summary.capacity)) +
          (summary.creationAvailable ? " · AVAILABLE" : " · ACTIVE AUDIO LIMIT") +
          "</span>"
        : "") +
      '<span class="overview-routing">' +
      routeText +
      "</span>" +
      "</header>"
    );
  }

  function overviewSummaryForControl(section, control) {
    var summaries = (section && section.controlSummaries) || [];
    var wanted = JSON.stringify((control && control.path) || null);
    for (var i = 0; i < summaries.length; i += 1) {
      if (JSON.stringify(summaries[i].controlPath || null) === wanted) {
        return summaries[i];
      }
    }
    return null;
  }

  function overviewControlHtml(control, summary, mode) {
    var state = controlState(control, mode);
    var focus = focusTreatment(control, mode);
    var id = controlIdOf(control);
    var focusPath = JSON.stringify(control.path || null);
    var parameterCount =
      summary && typeof summary.parameterCount === "number"
        ? summary.parameterCount
        : null;
    var summaryText =
      parameterCount === null
        ? ""
        : String(parameterCount) + (parameterCount === 1 ? " PARAM" : " PARAMS");
    var mark = stateMarkHtml(control, state);
    return (
      '<article class="overview-control" data-control="' +
      escapeHtml(id) +
      '" data-focus-path="' +
      escapeHtml(focusPath) +
      '" data-state="' +
      escapeHtml(state.name) +
      '" data-focus-treatment="' +
      (focus || "none") +
      '">' +
      '<div class="overview-control-label">' +
      '<span class="type-label">' +
      escapeHtml(String(control.label)) +
      "</span>" +
      (mark
        ? '<span class="type-hint" data-role="state-mark">' + mark + "</span>"
        : "") +
      "</div>" +
      '<div class="overview-control-value type-heading">' +
      escapeHtml(controlValueText(control)) +
      "</div>" +
      (summaryText
        ? '<div class="overview-control-summary type-hint muted" data-role="parameter-summary">' +
          escapeHtml(summaryText) +
          "</div>"
        : "") +
      lifecycleHtml(control, id) +
      "</article>"
    );
  }

  function patchOverviewHtml(model) {
    var main = surfaceById(model, "patchMain");
    var sections = (main && main.sections) || [];
    var mode = model.interactionMode;
    var sectionHtml = "";
    for (var sectionIndex = 0; sectionIndex < sections.length; sectionIndex += 1) {
      var section = sections[sectionIndex];
      var controlHtml = "";
      var paths = section.controlPaths || [];
      for (var pathIndex = 0; pathIndex < paths.length; pathIndex += 1) {
        var control = controlByPath(main, paths[pathIndex]);
        if (!control || !control.visible) {
          continue;
        }
        controlHtml += overviewControlHtml(
          control,
          overviewSummaryForControl(section, control),
          mode
        );
      }
      sectionHtml +=
        '<section class="overview-section" data-overview-section="' +
        escapeHtml(String(section.id)) +
        '"><div class="overview-section-heading">' +
        '<h2 class="type-label">' +
        escapeHtml(String(section.label)) +
        "</h2>" +
        '<span class="type-hint muted">' +
        String(paths.length) +
        (paths.length === 1 ? " CONTROL" : " CONTROLS") +
        "</span></div>" +
        '<div class="overview-section-controls">' +
        controlHtml +
        "</div></section>";
    }
    return (
      '<div class="patch-overview" id="patch-overview" data-role="patch-overview">' +
      overviewHeaderHtml(model, main) +
      '<div class="overview-sections">' +
      sectionHtml +
      "</div></div>"
    );
  }

  // The subordinate detail surface as the declared `CapabilityDetailShell`:
  // one composition arranging whichever capability the model names.
  //
  // It branches on no subject kind. The title is the projected value of the
  // exact Overview origin retained by the reducer's return path.
  //
  // The semantic model supplies the descriptor-authored sections and their
  // control paths. Section and row order therefore remain projected facts;
  // this shell only arranges them.
  function hasWaveform(surface) {
    return Boolean(surface && (surface.visualizations || []).some(function (visualization) {
      return visualization.data && visualization.data.kind === "waveform";
    }));
  }

  function detailShellHtml(model) {
    var detail = surfaceByRole(model, "detail");
    var controls = (detail && detail.controls) || [];
    var mode = model.interactionMode;
    var main = surfaceById(model, "patchMain");
    var head = controlByPath(main, model.returnPath && model.returnPath.origin);
    var subject = head ? controlValueText(head) : UNAVAILABLE_MARK;
    var summary = (detail && detail.summary) || {};
    var subjectSummary = summary.subject || {};
    var subjectKind = String(subjectSummary.kind || "unknown");
    var mainSummary = (main && main.summary) || {};
    var prospective = mainSummary.kind === "emptyPatch";
    var patchName = prospective ? String(mainSummary.activeCount).padStart(2, "0") + " / DEFAULT" : String(mainSummary.patchName || UNAVAILABLE_MARK);
    var patchId = prospective ? null : mainSummary.patchId;
    var patchPosition = prospective ? summary.patchPosition : patchId;
    var originControl = controlIdOf({
      path: model.returnPath && model.returnPath.origin,
    });
    var slotPosition = null;
    var slotPrefix = "patch.effectSlot.";
    if (originControl.indexOf(slotPrefix) === 0) {
      var parsedSlot = Number(originControl.slice(slotPrefix.length));
      if (Number.isInteger(parsedSlot) && parsedSlot >= 0) {
        slotPosition = parsedSlot;
      }
    }
    var visibleControlCount = controls.filter(function (control) {
      return control.visible === true;
    }).length;
    // A capability mid-preparation reports its projected lifecycle word here
    // as well as on its rows, so the shell is never read as a settled one.
    var status = head && head.status ? head.status : null;
    for (var s = 0; !status && s < controls.length; s += 1) {
      if (controls[s].status) {
        status = controls[s].status;
        break;
      }
    }
    var sections = "";
    var leadingControls = "";
    var declaredSections = (detail && detail.sections) || [];
    var detailOrdinal = 0;
    for (var sectionIndex = 0; sectionIndex < declaredSections.length; sectionIndex += 1) {
      var declared = declaredSections[sectionIndex];
      var sectionRows = "";
      var paths = declared.controlPaths || [];
      for (var pathIndex = 0; pathIndex < paths.length; pathIndex += 1) {
        var wanted = JSON.stringify(paths[pathIndex]);
        for (var rowIndex = 0; rowIndex < controls.length; rowIndex += 1) {
          if (controls[rowIndex].visible && JSON.stringify(controls[rowIndex].path) === wanted) {
            sectionRows += patchRowHtml(
              controls[rowIndex],
              mode,
              "detail",
              detailOrdinal
            );
            detailOrdinal += 1;
            break;
          }
        }
      }
      var sectionMarkup =
        '<section class="detail-section" data-role="detail-section" data-detail-section="' +
        escapeHtml(String(declared.id)) +
        '"><div class="detail-section-heading"><h3 class="type-label muted" data-role="detail-section-label">' +
        escapeHtml(String(declared.label)) +
        '</h3><span class="type-hint muted" data-role="detail-section-count">' +
        String(paths.length) +
        (paths.length === 1 ? " PARAM" : " PARAMS") +
        "</span></div>" +
        (sectionRows || markUnavailableRowHtml(String(declared.label))) +
        "</section>";
      if (declared.leadingControls) leadingControls += sectionMarkup;
      else sections += sectionMarkup;
    }
    if (!sections) {
      var rows = "";
      for (var i = 0; i < controls.length; i += 1) {
        if (controls[i].visible) {
          rows += patchRowHtml(controls[i], mode, "detail", detailOrdinal);
          detailOrdinal += 1;
        }
      }
      sections =
        '<section class="detail-section" data-detail-section="unsectioned">' +
        (rows || markUnavailableRowHtml(String((detail && detail.label) || "DETAIL"))) +
        "</section>";
    }
    var visualizationMarkup = "";
    var waveformMarkup = "";
    var visualizations = (detail && detail.visualizations) || [];
    for (var visualizationIndex = 0; visualizationIndex < visualizations.length; visualizationIndex += 1) {
      var visualization = visualizations[visualizationIndex];
      if (visualization.data && visualization.data.kind === "waveform") {
        waveformMarkup += waveformAssetHtml(visualization.data, controls) +
          detailVisualizationHtml(visualization);
      } else {
        visualizationMarkup += detailVisualizationHtml(visualization);
      }
    }
    return (
      '<div class="detail' + (waveformMarkup ? " has-waveform" : "") +
      '" id="detail" data-detail-kind="' +
      escapeHtml(subjectKind) +
      '" data-patch-position="' +
      escapeHtml(String(patchPosition)) +
      '"' +
      (patchId === null
        ? ""
        : ' data-patch-id="' + escapeHtml(String(patchId)) + '"') +
      ' data-origin-control="' +
      escapeHtml(originControl) +
      '"' +
      (slotPosition === null
        ? ""
        : ' data-slot-position="' + String(slotPosition) + '"') +
      ">" +
      leadingControls + waveformMarkup +
      '<header class="detail-title detail-header" data-role="detail-title">' +
      '<span class="type-label muted">' +
      escapeHtml(String((detail && detail.label) || "DETAIL")) +
      "</span>" +
      '<span class="detail-kind type-heading" data-role="detail-kind">' +
      escapeHtml(
        subjectKind === "effect"
          ? "POST FX " +
              String((slotPosition === null ? 0 : slotPosition) + 1).padStart(2, "0")
          : subjectKind.toUpperCase()
      ) +
      "</span>" +
      '<span class="type-heading focus" aria-hidden="true">/</span>' +
      '<span class="type-heading focus" data-role="detail-subject">' +
      escapeHtml(subject) +
      "</span>" +
      '<span class="spring"></span>' +
      '<span class="type-hint secondary" data-role="detail-patch">' +
      (prospective ? "PATCH " + String(mainSummary.activeCount).padStart(2, "0") + " · DEFAULT" : "PATCH " + escapeHtml(String(patchId))) +
      HINT_SEPARATOR +
      escapeHtml(patchName) +
      "</span>" +
      '<span class="type-hint muted" data-role="detail-control-count">' +
      String(visibleControlCount) +
      (visibleControlCount === 1 ? " PARAM" : " PARAMS") +
      "</span>" +
      (status
        ? '<span class="type-hint adjust" data-role="detail-status">' +
          escapeHtml(String(status.label)) +
          "</span>"
        : "") +
      "</header>" +
      '<div class="detail-sections" data-role="detail-sections">' +
      sections +
      "</div>" +
      '<div class="detail-visualizations" data-role="detail-visualizations">' +
      visualizationMarkup +
      "</div></div>"
    );
  }

  // The projected asset supplies identity; the associated control retains its
  // sole focus target in descriptor order below the informative waveform.
  function waveformAssetHtml(data, controls) {
    var asset = data.asset;
    var control = controls.find(function (candidate) {
      return candidate.value && candidate.value.kind === "asset" &&
        asset && candidate.value.value.locator === asset.locator;
    });
    return '<div class="detail-asset" data-role="detail-asset">' +
      '<span class="type-label" data-role="detail-asset-label">' +
      escapeHtml(control ? String(control.label) : "ASSET") +
      '</span><span class="type-value" data-role="detail-asset-path">' +
      escapeHtml(asset ? String(asset.locator) : "EMPTY") +
      '</span><span class="type-hint" data-role="detail-asset-status">' +
      escapeHtml(String(data.status || UNAVAILABLE)) + '</span></div>';
  }

  function waveformHtml(data) {
    var pairs = data.pairs || [];
    var compatible = data.asset && Number.isInteger(data.frames) && data.frames > 0 &&
      Number.isInteger(data.sampleRate) && data.sampleRate > 0 && pairs.length > 0 &&
      pairs.every(function (pair) {
        return [pair.leftMin, pair.leftMax, pair.rightMin, pair.rightMax].every(function (value) {
          return typeof value === "number" && Number.isFinite(value) && value >= -1 && value <= 1;
        }) && pair.leftMin <= pair.leftMax && pair.rightMin <= pair.rightMax;
      });
    var bars = "";
    var markers = "";
    var legend = "";
    if (compatible) {
      // Aggregate every prepared pair in each display bin, across both channels.
      // Silent bins have zero height; no minimum amplitude is manufactured.
      var stride = Math.max(1, Math.ceil(pairs.length / 192));
      for (var index = 0; index < pairs.length; index += stride) {
        var low = 1;
        var high = -1;
        for (var sample = index; sample < Math.min(index + stride, pairs.length); sample += 1) {
          low = Math.min(low, pairs[sample].leftMin, pairs[sample].rightMin);
          high = Math.max(high, pairs[sample].leftMax, pairs[sample].rightMax);
        }
        bars += '<i data-wave-top="' + ((1 - high) / 2).toFixed(6) +
          '" data-wave-height="' + ((high - low) / 2).toFixed(6) + '"></i>';
      }
      var labels = {
        playbackStart: "START", playbackEnd: "END",
        loopStart: "LOOP IN", loopEnd: "LOOP OUT"
      };
      var landmarks = data.landmarks || [];
      for (var landmarkIndex = 0; landmarkIndex < landmarks.length; landmarkIndex += 1) {
        var landmark = landmarks[landmarkIndex];
        var position = landmark.normalizedPosition;
        if (typeof position !== "number" || !Number.isFinite(position) || position < 0 || position > 1) {
          continue;
        }
        var label = labels[landmark.role] || String(landmark.role);
        markers += '<b data-landmark="' + escapeHtml(String(landmark.role)) +
          '" data-landmark-position="' + position.toFixed(6) +
          '" aria-label="' + escapeHtml(label) + '"></b>';
        legend += '<span data-landmark-label="' + escapeHtml(String(landmark.role)) +
          '">' + escapeHtml(label) + ' ' + compactNumericText(position * 100, 0.1) + '%</span>';
      }
    }
    var status = String(data.status || UNAVAILABLE);
    return '<div class="waveform-region" data-waveform-state="' +
      (compatible ? "available" : "unavailable") + '">' +
      '<div class="waveform-legend type-hint" data-role="waveform-legend">' +
      (legend || (compatible ? "LANDMARKS UNAVAILABLE" : "")) + '</div>' +
      '<div class="waveform-shape" data-role="waveform-shape">' +
      (compatible ? '<div class="waveform-bars">' + bars + '</div>' + markers :
        '<span class="type-label muted">WAVEFORM UNAVAILABLE</span>') +
      '</div><div class="waveform-reading type-hint">' +
      '<span data-role="waveform-status">' + escapeHtml(status) + '</span>' +
      (compatible ? '<span data-role="waveform-metadata">' +
        escapeHtml(String(data.sampleRate)) + ' Hz · ' + escapeHtml(String(data.channels)) +
        ' CH · ' + compactNumericText(data.frames / data.sampleRate, 0.001) + ' s</span>' : '') +
      '</div></div>';
  }

  function detailVisualizationHtml(visualization) {
    var data = visualization.data || { kind: "status", text: UNAVAILABLE };
    var body = "";
    if (data.kind === "envelope") {
      var attackMaximumMilliseconds = finiteClampedNumber(
        data.attackMaximumMilliseconds,
        1,
        Number.MAX_SAFE_INTEGER
      );
      var attackMilliseconds = finiteClampedNumber(
        data.attackMilliseconds,
        0,
        attackMaximumMilliseconds
      );
      var decayMaximumMilliseconds = finiteClampedNumber(
        data.decayMaximumMilliseconds,
        1,
        Number.MAX_SAFE_INTEGER
      );
      var decayMilliseconds = finiteClampedNumber(
        data.decayMilliseconds,
        0,
        decayMaximumMilliseconds
      );
      var sustain = finiteClampedNumber(data.sustain, 0, 1);
      var releaseMaximumMilliseconds = finiteClampedNumber(
        data.releaseMaximumMilliseconds,
        1,
        Number.MAX_SAFE_INTEGER
      );
      var releaseMilliseconds = finiteClampedNumber(
        data.releaseMilliseconds,
        0,
        releaseMaximumMilliseconds
      );
      body = envelopeSvgHtml({
        attackMilliseconds: attackMilliseconds,
        attackMaximumMilliseconds: attackMaximumMilliseconds,
        decayMilliseconds: decayMilliseconds,
        decayMaximumMilliseconds: decayMaximumMilliseconds,
        sustain: sustain,
        releaseMilliseconds: releaseMilliseconds,
        releaseMaximumMilliseconds: releaseMaximumMilliseconds,
      });
    } else if (data.kind === "waveform") {
      body = waveformHtml(data);
    } else {
      body = '<span class="type-hint">' + escapeHtml(String(data.text || UNAVAILABLE)) + "</span>";
    }
    return (
      '<figure class="detail-visualization" data-focusable="false" aria-label="non-interactive visualization" data-visualization="' +
      escapeHtml(String(visualization.id)) +
      '" data-visualization-kind="' +
      escapeHtml(String(data.kind)) +
      '"><figcaption class="type-label muted">' +
      escapeHtml(String(visualization.label)) +
      "</figcaption>" +
      body +
      "</figure>"
    );
  }

  // A finite projected scalar constrained to its declared envelope bounds.
  // Invalid transport data never reaches SVG geometry as NaN/Infinity.
  function finiteClampedNumber(value, minimum, maximum) {
    var number = Number(value);
    if (!Number.isFinite(number)) {
      number = minimum;
    }
    return Math.max(minimum, Math.min(maximum, number));
  }

  // The parameterized ADSR SVG keeps real milliseconds in its data/readout,
  // but compresses each timed phase logarithmically for display. Envelope
  // times span several orders of magnitude, so a direct 31-second axis made
  // musically meaningful short values disappear. Every timed phase can
  // consume one stable graphical span at its projected canonical maximum;
  // Sustain consumes one equal, explicitly non-temporal preview span. Because
  // the scale is fixed rather than normalized against the current values,
  // changing one phase never rescales any other phase.
  var ENVELOPE_PHASE_SPAN_UNITS = 1000;
  var ENVELOPE_SUSTAIN_PREVIEW_SPAN_UNITS = 1000;
  var ENVELOPE_VIEWBOX_WIDTH_UNITS =
    ENVELOPE_PHASE_SPAN_UNITS * 3 +
    ENVELOPE_SUSTAIN_PREVIEW_SPAN_UNITS;

  function envelopeSvgHtml(parameters) {
    var geometry = envelopeSvgGeometry(parameters);
    var millisecondsLabel =
      "A " +
      formatEnvelopeMilliseconds(parameters.attackMilliseconds) +
      " / D " +
      formatEnvelopeMilliseconds(parameters.decayMilliseconds) +
      " / S " +
      formatEnvelopeSustain(parameters.sustain) +
      " / R " +
      formatEnvelopeMilliseconds(parameters.releaseMilliseconds);
    return (
      '<svg class="envelope-shape" data-role="envelope-shape" aria-hidden="true"' +
      ' viewBox="0 0 ' +
      String(ENVELOPE_VIEWBOX_WIDTH_UNITS) +
      ' 1000" preserveAspectRatio="none"' +
      ' data-time-coordinate="log1p-milliseconds"' +
      ' data-attack-maximum-milliseconds="' +
      parameters.attackMaximumMilliseconds.toFixed(3) +
      '" data-decay-maximum-milliseconds="' +
      parameters.decayMaximumMilliseconds.toFixed(3) +
      '" data-release-maximum-milliseconds="' +
      parameters.releaseMaximumMilliseconds.toFixed(3) +
      '" data-phase-span-units="' +
      String(ENVELOPE_PHASE_SPAN_UNITS) +
      '" data-sustain-preview-span-units="' +
      String(ENVELOPE_SUSTAIN_PREVIEW_SPAN_UNITS) +
      '" data-viewbox-width-units="' +
      String(ENVELOPE_VIEWBOX_WIDTH_UNITS) +
      '" data-attack-milliseconds="' +
      parameters.attackMilliseconds.toFixed(3) +
      '" data-decay-milliseconds="' +
      parameters.decayMilliseconds.toFixed(3) +
      '" data-sustain="' +
      parameters.sustain.toFixed(6) +
      '" data-release-milliseconds="' +
      parameters.releaseMilliseconds.toFixed(3) +
      '" data-stage-points="' +
      geometry.points +
      '">' +
      '<line class="envelope-baseline" x1="0" y1="960" x2="' +
      String(ENVELOPE_VIEWBOX_WIDTH_UNITS) +
      '" y2="960"></line>' +
      '<path class="envelope-path" data-role="envelope-path" d="' +
      geometry.path +
      '"></path>' +
      geometry.nodes +
      "</svg>" +
      '<span class="type-hint" data-role="envelope-values">' +
      millisecondsLabel +
      "</span>"
    );
  }

  function envelopeTimeSpanUnits(milliseconds, maximumMilliseconds) {
    var clamped = finiteClampedNumber(
      milliseconds,
      0,
      maximumMilliseconds
    );
    return (
      ENVELOPE_PHASE_SPAN_UNITS *
      (Math.log1p(clamped) / Math.log1p(maximumMilliseconds))
    );
  }

  function envelopeSvgGeometry(parameters) {
    var peakY = 40;
    var baselineY = 960;
    var nodeRadius = 24;
    var sustainY =
      baselineY - (baselineY - peakY) * parameters.sustain;
    var attackEndX = envelopeTimeSpanUnits(
      parameters.attackMilliseconds,
      parameters.attackMaximumMilliseconds
    );
    var decayEndX =
      attackEndX +
      envelopeTimeSpanUnits(
        parameters.decayMilliseconds,
        parameters.decayMaximumMilliseconds
      );
    var releaseStartX =
      decayEndX + ENVELOPE_SUSTAIN_PREVIEW_SPAN_UNITS;
    var releaseEndX =
      releaseStartX +
      envelopeTimeSpanUnits(
        parameters.releaseMilliseconds,
        parameters.releaseMaximumMilliseconds
      );
    var points = [
      [0, baselineY],
      [attackEndX, peakY],
      [decayEndX, sustainY],
      [releaseStartX, sustainY],
      [releaseEndX, baselineY],
      [ENVELOPE_VIEWBOX_WIDTH_UNITS, baselineY],
    ];
    var serializedPoints = points
      .map(function (point) {
        return point[0].toFixed(3) + "," + point[1].toFixed(3);
      })
      .join(" ");
    var path = points
      .map(function (point, index) {
        return (
          (index === 0 ? "M " : "L ") +
          point[0].toFixed(3) +
          " " +
          point[1].toFixed(3)
        );
      })
      .join(" ");
    var nodes = points
      .slice(1, 5)
      .map(function (point) {
        return (
          '<circle class="envelope-node" cx="' +
          point[0].toFixed(3) +
          '" cy="' +
          point[1].toFixed(3) +
          '" r="' +
          String(nodeRadius) +
          '"></circle>'
        );
      })
      .join("");
    return { path: path, points: serializedPoints, nodes: nodes };
  }

  function formatEnvelopeMilliseconds(milliseconds) {
    var rounded = Math.round(milliseconds);
    var text =
      Math.abs(milliseconds - rounded) < 0.0005
        ? String(rounded)
        : milliseconds.toFixed(3).replace(/0+$/, "").replace(/\.$/, "");
    return text + " ms";
  }

  function formatEnvelopeSustain(sustain) {
    var percentage = sustain * 100;
    var rounded = Math.round(percentage);
    return (
      (Math.abs(percentage - rounded) < 0.0005
        ? String(rounded)
        : percentage.toFixed(1).replace(/\.0$/, "")) + "%"
    );
  }

  // Reads a serialized SemanticControlId/PatchControlId without inventing a
  // second identity vocabulary. The stable id is used only for structure and
  // data attributes; it is never painted as a display name.
  function serializedControlId(value) {
    if (value && typeof value === "object" && value.id !== undefined) {
      value = value.id;
    }
    return value === null || value === undefined
      ? ""
      : typeof value === "object"
        ? JSON.stringify(value)
        : String(value);
  }

  function optionRequestedText(control) {
    if (!control || !control.requestedValue) {
      return UNAVAILABLE_MARK;
    }
    return controlValueText({
      kind: control.kind,
      value: control.requestedValue,
      selectedLabel: control.requestedLabel,
      numericRange: control.numericRange,
      unit: control.unit,
    });
  }

  // The option surface's exact active/requested structural truth stays
  // anchored to the origin surface control named by its projected subject. It is
  // a readout, never a second focus target. Ready, in-flight, unavailable,
  // and failed all use the same fields and differ through explicit text,
  // data state, and keyline treatment rather than color alone.
  function optionOriginStatusHtml(origin) {
    if (!origin) {
      return (
        '<div class="option-origin-status is-unavailable" data-role="option-origin-status" data-lifecycle="unavailable">' +
        '<span class="type-hint warning">ORIGIN ' +
        UNAVAILABLE +
        "</span></div>"
      );
    }
    var status = origin.status || {};
    var lifecycle = String(status.kind || "ready");
    var active = controlValueText(origin);
    var requested = optionRequestedText(origin);
    var cause = origin.error ? String(origin.error.label) : "";
    var activeRevision =
      status.graphRevision === null || status.graphRevision === undefined
        ? UNAVAILABLE_MARK
        : String(status.graphRevision);
    var requestedRevision =
      status.targetGraphRevision === null ||
      status.targetGraphRevision === undefined
        ? UNAVAILABLE_MARK
        : String(status.targetGraphRevision);
    var stateLabel = String(status.label || lifecycle).toUpperCase();
    return (
      '<div class="option-origin-status state-' +
      escapeHtml(lifecycle) +
      (cause ? " has-cause" : "") +
      '" data-role="option-origin-status" data-lifecycle="' +
      escapeHtml(lifecycle) +
      '">' +
      '<span class="type-hint muted" data-role="option-origin-label">ORIGIN ' +
      escapeHtml(String(origin.label || UNAVAILABLE_MARK).toUpperCase()) +
      "</span>" +
      '<span class="type-hint" data-role="option-active">ACTIVE ' +
      escapeHtml(active) +
      "</span>" +
      '<span class="type-hint adjust" data-role="option-requested">REQUESTED ' +
      escapeHtml(requested) +
      "</span>" +
      '<span class="type-hint option-lifecycle-state" data-role="option-lifecycle">' +
      escapeHtml(stateLabel) +
      "</span>" +
      '<span class="type-hint secondary" data-role="option-revisions">GRAPH ' +
      escapeHtml(activeRevision) +
      " → " +
      escapeHtml(requestedRevision) +
      "</span>" +
      (cause
        ? '<span class="type-hint warning" data-role="option-cause">CAUSE ' +
          escapeHtml(cause) +
          "</span>"
        : "") +
      "</div>"
    );
  }

  // One shared subordinate modal entry point with deliberately separate
  // compositions for descriptor choices and Sample Browser. The browser
  // keeps its waveform/preview behavior; Engine and Post FX share the one
  // Figma-authored option hierarchy below. Neither branch owns an option or
  // file index.
  function choiceOptionRowsHtml(controls) {
    var optionRows = "";
    for (var optionIndex = 0; optionIndex < controls.length; optionIndex += 1) {
      var option = controls[optionIndex];
      if (option.visible !== true) {
        continue;
      }
      var current = String(option.selectedLabel || "") === "CURRENT";
      var availability = option.availabilityLabel
        ? "unavailable"
        : option.enabled
          ? "available"
          : "disabled";
      var availabilityText =
        availability === "unavailable"
          ? "UNAVAILABLE" + HINT_SEPARATOR + String(option.availabilityLabel)
          : availability.toUpperCase();
      var optionIdentity =
        option.value && option.value.kind === "identity"
          ? String(option.value.value)
          : "";
      optionRows +=
        '<div class="modal-option option-row' +
        (option.focused ? " is-focused" : "") +
        (current ? " is-current" : "") +
        (!option.enabled ? " is-disabled" : "") +
        (availability === "unavailable" ? " is-unavailable" : "") +
        '" data-focus-path="' +
        escapeHtml(JSON.stringify(option.path || null)) +
        '" data-option-id="' +
        escapeHtml(optionIdentity) +
        '" data-option-state="' +
        availability +
        '" data-enabled="' +
        String(option.enabled === true) +
        '" data-current="' +
        String(current) +
        '" data-valid-actions="' +
        escapeHtml(JSON.stringify(option.validActions || [])) +
        '">' +
        '<span class="modal-option-shape" aria-hidden="true">' +
        (option.focused ? "&gt;" : "&nbsp;") +
        "</span>" +
        '<span class="type-value modal-option-label">' +
        escapeHtml(String(option.label || UNAVAILABLE_MARK)) +
        "</span>" +
        '<span class="type-hint modal-option-availability">' +
        escapeHtml(availabilityText) +
        "</span>" +
        (current
          ? '<span class="type-hint muted modal-option-current"><span aria-hidden="true">■</span> CURRENT</span>'
          : "") +
        "</div>";
    }
    if (!optionRows) {
      optionRows = markUnavailableRowHtml("OPTIONS");
    }
    return optionRows;
  }

  function modalShellHtml(model) {
    var modal = surfaceByRole(model, "modal");
    if (!modal) {
      return "";
    }
    var summary = modal.summary || {};
    var browser = summary.kind === "fileBrowser";
    var controls = modal.controls || [];
    if (browser) {
      var browserRows = "";
      for (var browserRowIndex = 0; browserRowIndex < controls.length; browserRowIndex += 1) {
        var browserControl = controls[browserRowIndex];
        if (browserControl.visible !== true) {
          continue;
        }
        var browserMarker = [];
        if (browserControl.selectedLabel) {
          browserMarker.push(String(browserControl.selectedLabel));
        }
        if (browserControl.focused) {
          browserMarker.push("FOCUS");
        }
        if (!browserControl.enabled) {
          browserMarker.push(UNAVAILABLE);
        }
        var browserMetadata = browserControl.browserMetadata || null;
        if (browserMetadata && browserMetadata.status === "pending") {
          browserMarker.push("LOADING");
        } else if (browserMetadata && browserMetadata.status === "unavailable") {
          browserMarker.push(UNAVAILABLE);
        } else if (browserMetadata && browserMetadata.status === "failed") {
          browserMarker.push("INVALID");
        }
        browserRows +=
          '<div class="modal-option' +
          (browserControl.focused ? " is-focused" : "") +
          (!browserControl.enabled ? " is-disabled" : "") +
          (browserMetadata ? " has-browser-metadata" : "") +
          '" data-focus-path="' +
          escapeHtml(JSON.stringify(browserControl.path || null)) +
          '" data-metadata-state="' +
          escapeHtml(String((browserMetadata && browserMetadata.status) || "none")) +
          '">' +
          '<span class="modal-option-shape" aria-hidden="true">' +
          (browserControl.focused ? "▶" : "◇") +
          "</span>" +
          '<span class="type-value modal-option-label">' +
          escapeHtml(String(browserControl.label || UNAVAILABLE_MARK)) +
          "</span>" +
          (browserMetadata || browserControl.unit
            ? '<span class="type-hint muted modal-option-meta">' +
              escapeHtml(
                String(
                  browserMetadata ? browserMetadata.text : browserControl.unit
                )
              ) +
              "</span>"
            : "") +
          '<span class="type-hint modal-option-state">' +
          escapeHtml(browserMarker.join(HINT_SEPARATOR) || "AVAILABLE") +
          "</span></div>";
      }
      if (!browserRows) {
        browserRows = markUnavailableRowHtml("FILES");
      }
      var browserVisualizations = "";
      var modalVisualizations = modal.visualizations || [];
      var previewAvailable = hasWaveform(modal);
      for (var visualizationIndex = 0; visualizationIndex < modalVisualizations.length; visualizationIndex += 1) {
        browserVisualizations += detailVisualizationHtml(modalVisualizations[visualizationIndex]);
      }
      var preview = summary.preview || { kind: "idle" };
      var previewText = String(preview.kind || "idle").toUpperCase();
      if (preview.assetId) {
        previewText += HINT_SEPARATOR + String(preview.assetId);
      }
      if (preview.kind === "preparing") {
        previewText += preview.held ? HINT_SEPARATOR + "HELD" : HINT_SEPARATOR + "RELEASED";
      }
      var browserStatus =
        '<div class="browser-status" data-role="browser-status">' +
        '<span class="type-label muted">' +
        escapeHtml(String(summary.folder || "/")) +
        "</span>" +
        '<span class="type-hint">' +
        escapeHtml(String(summary.lifecycle || UNAVAILABLE).toUpperCase()) +
        "</span>" +
        (previewAvailable ? '<span class="type-hint adjust">PREVIEW ' +
        escapeHtml(previewText) +
        "</span>" +
        '<span class="type-hint muted">ORIGIN PATCH ROUTING / MUTE / SOLO / SENDS APPLY</span>' : "") +
        "</div>";
      return (
        '<div class="modal-shell sample-browser" id="modal-shell">' +
        '<div class="modal-title">' +
        '<span class="type-label muted">LIBRARY</span>' +
        '<span class="type-heading focus">' +
        escapeHtml(String(modal.label || "FILE BROWSER")) +
        "</span></div>" +
        browserStatus +
        (previewAvailable ? '<div class="browser-visualizations" data-role="browser-visualizations">' +
        (browserVisualizations || markUnavailableRowHtml("WAVEFORM")) +
        "</div>" +
        '<div class="preview-region" data-role="preview-region" data-preview-state="' +
        escapeHtml(String(preview.kind || "idle")) +
        '" aria-label="preview ' +
        escapeHtml(previewText.toLowerCase()) +
        '">' +
        '<span class="type-label muted">PREVIEW</span>' +
        '<span class="preview-playhead-shape" aria-hidden="true"><span class="preview-playhead-marker"></span></span>' +
        '<span class="type-hint" data-role="preview-playhead">PLAYHEAD — / ' +
        escapeHtml(String(preview.kind || "idle").toUpperCase()) +
        "</span></div>" : "") +
        '<div class="modal-options" data-role="modal-options">' +
        browserRows +
        "</div></div>"
      );
    }

    var summary = modal.summary || {};
    var subject = summary.subject || {};
    var subjectControl = serializedControlId(subject.controlId);
    var originPath = model.returnPath && model.returnPath.origin;
    var originSurface = surfaceById(model, originPath && originPath.surface);
    var origin = controlById(originSurface, subjectControl);
    var returnControl = serializedControlId(
      model.returnPath && model.returnPath.origin
        ? model.returnPath.origin.controlId
        : null
    );
    var slotMatch = /^patch\.effectSlot\.(\d+)$/.exec(subjectControl);
    var optionKind =
      subjectControl === "patch.engine"
        ? "engine"
        : slotMatch
          ? "postFx"
          : "generic";
    var slotPosition = slotMatch ? Number(slotMatch[1]) : null;
    var title =
      optionKind === "engine"
        ? "ENGINE TYPE OPTIONS"
        : optionKind === "postFx"
          ? "POST FX OPTIONS"
          : String(modal.label || "OPTIONS").toUpperCase();
    var sourceAnnotation =
      optionKind === "engine"
        ? "Opened from Patch / Instrument selector"
        : optionKind === "postFx"
          ? "Opened from Patch / Post FX selector · SLOT " +
            String(slotPosition + 1).padStart(2, "0")
          : "Opened from Patch / " +
            String((origin && origin.label) || "selector");
    if (returnControl && returnControl !== subjectControl) {
      var returnOrigin = controlById(originSurface, returnControl);
      sourceAnnotation +=
        " · RETURN REPAIRED TO " +
        String((returnOrigin && returnOrigin.label) || UNAVAILABLE_MARK).toUpperCase();
    }

    var optionRows = choiceOptionRowsHtml(controls);
    var categoryLabel = summary.choiceGroupLabel;
    var groupHeader = categoryLabel
      ? '<div class="option-group" data-role="choice-group">' +
        '<span class="type-hint muted">A ‹</span>' +
        '<span class="type-label" data-role="choice-group-label">' +
        escapeHtml(String(categoryLabel).toUpperCase()) +
        '</span><span class="type-hint muted">› D</span>' +
        '<span class="type-hint muted option-group-help">A / D groups · W / S ' +
        (optionKind === "postFx" ? "effects" : "instruments") + '</span></div>'
      : "";
    return (
      '<div class="modal-shell option-modal" id="modal-shell" data-option-kind="' +
      escapeHtml(optionKind) +
      '" data-subject-control="' +
      escapeHtml(subjectControl) +
      '" data-return-control="' +
      escapeHtml(returnControl) +
      '"' +
      (slotPosition === null
        ? ""
        : ' data-slot-position="' + String(slotPosition) + '"') +
      ">" +
      '<header class="option-header" data-role="option-header">' +
      '<span class="type-heading option-title" data-role="option-title">' +
      escapeHtml(title) +
      "</span>" +
      "</header>" +
      '<div class="type-hint muted option-source" data-role="option-source">' +
      escapeHtml(sourceAnnotation) +
      "</div>" +
      optionOriginStatusHtml(origin) +
      groupHeader +
      '<div class="option-rule" aria-hidden="true"></div>' +
      '<div class="modal-options" data-role="modal-options">' +
      optionRows +
      "</div></div>"
    );
  }

  // The PATCH main workspace: the section header content (the surface's own
  // label left, the focused entry's annotation right) on the shared caption
  // row, then the composition the document's own surface set selects — the
  // detail shell while a detail entry is open, the Overview otherwise.
  // The shell bands and the persistent side region are untouched either way.
  function patchWorkspaceHtml(model) {
    var main = surfaceById(model, "patchMain");
    var detail = surfaceByRole(model, "detail");
    var modal = surfaceByRole(model, "modal");
    var body = modal
      ? modalShellHtml(model)
      : detail
        ? detailShellHtml(model)
        : patchOverviewHtml(model);
    var focused = focusedControl(model);
    var annotation = focused
      ? '<span class="type-hint focus" data-role="section-annotation">FOCUS' +
        HINT_SEPARATOR +
        escapeHtml(String(focused.label)) +
        "</span>"
      : "";
    var surface = modal || detail || main;
    var title = escapeHtml(String((surface && surface.label) || "PATCH"));
    return workspaceScaffold(
      model,
      '<span class="type-label muted">' + title + "</span>",
      annotation,
      body
    );
  }

  function sendsWorkspaceHtml(model) {
    var surface = surfaceById(model, "sends");
    if (surfaceByRole(model, "modal")) {
      return workspaceScaffold(model, '<span class="type-label muted">FILE BROWSER</span>', "", modalShellHtml(model));
    }
    var summary = surface.summary;
    var controls = surface.controls || [];
    var body = '<div class="send-shell"><div class="send-heading"><span class="type-hint muted">Q ‹</span><h2 class="type-heading">SEND ' + String(summary.bus + 1).padStart(2, "0") + ' / ' + String(summary.count) + '</h2><span class="type-hint muted">› E</span></div><div class="type-value send-name">' + escapeHtml(summary.name) + '</div>';
    if (summary.choosingEffect) {
      body += '<div class="option-group"><span class="type-hint muted">A ‹</span><span class="type-label">' + escapeHtml(summary.choiceGroupLabel || "Effects") + '</span><span class="type-hint muted">› D</span></div>';
      body += '<div class="type-hint muted">W / S effects · Return choose · Shift+Down cancel</div>';
      body += '<div class="modal-options">' + choiceOptionRowsHtml(controls) + '</div>';
    } else {
      surface.sections.forEach(function (section) {
        body += '<section class="detail-section"><h3 class="type-label muted detail-section-heading">' + escapeHtml(section.label) + '</h3>';
        section.controlPaths.forEach(function (path) {
          var control = controlByPath(surface, path);
          if (control && control.visible) { body += patchRowHtml(control, model.interactionMode, "detail"); }
        });
        body += '</section>';
      });
      if (summary.editingName) {
        body += '<form id="send-name-form" data-bus="' + String(summary.bus) + '"><label class="type-label" for="send-name-input">Send name</label><input id="send-name-input" autocomplete="off" required value="' + escapeHtml(summary.name) + '" /><span class="type-hint muted">Return save · Escape cancel</span></form>';
      }
    }
    body += '</div>';
    return workspaceScaffold(model, '<span class="type-label muted">SENDS</span>', '', body);
  }

  function attachSendNameInput(draft) {
    var form = window.document.getElementById("send-name-form");
    if (!form) { return; }
    var input = window.document.getElementById("send-name-input");
    if (draft && draft.bus === form.dataset.bus) { input.value = draft.value; }
    input.focus();
    if (draft && draft.bus === form.dataset.bus) { input.setSelectionRange(draft.start, draft.end); } else { input.select(); }
    function emit(action) { var tauri = window.__TAURI__; if (tauri && tauri.event) { tauri.event.emit("crest://send-name", action); } }
    form.addEventListener("submit", function (event) {
      event.preventDefault();
      var name = input.value.trim();
      if (!name || /[\u0000-\u001f\u007f]/.test(name)) { input.setCustomValidity("Enter a name without control characters."); input.reportValidity(); return; }
      input.setCustomValidity(""); emit({ kind: "rename", bus: Number(form.dataset.bus), name: name });
    });
    input.addEventListener("input", function () { input.setCustomValidity(""); });
    input.addEventListener("keydown", function (event) { if (event.key === "Escape") { event.preventDefault(); emit({ kind: "cancelRename" }); } });
  }

  // ---- the persistent side region -----------------------------------------

  // Dispatches on the document's own side-surface summary — the Inspector
  // reading for a mixerInspector summary, the Utility reading for a
  // patchUtility one. A missing side surface says so; an incoherent summary
  // is marked, never guessed at.
  function sideRegionHtml(model) {
    var side = persistentSideSurface(model);
    if (!side) {
      // The persistent side region with no surface behind it says so; it is
      // never painted with representative content.
      return (
        '<span class="type-label muted">CURSOR</span>' +
        '<span class="type-value muted" data-role="cursor">' +
        UNAVAILABLE +
        "</span>"
      );
    }
    var summary = side.summary || {};
    if (summary.kind === "mixerInspector") {
      return mixerInspectorHtml(model, side);
    }
    if (summary.kind === "patchUtility" || summary.kind === "emptyPatchUtility") {
      return patchUtilityHtml(model, side, summary);
    }
    // A main-surface summary in the side region is incoherent: the panel has
    // no reading for it and will not invent one.
    return (
      '<span class="type-label muted">PANEL SUMMARY</span>' +
      '<span class="type-value muted">' +
      UNAVAILABLE_MARK +
      "</span>"
    );
  }

  // The PATCH Utility panel follows the projected surface order. Labels,
  // identities, visibility, values, state, and actions all come from the
  // canonical document; the page owns no parallel entry registry.
  function patchUtilityHtml(model, surface, summary) {
    var mode = model.interactionMode;
    var controls = surface.controls || [];
    var rows = "";
    for (var i = 0; i < controls.length; i += 1) {
      if (!controls[i].visible) {
        continue;
      }
      rows += patchRowHtml(controls[i], mode, "panel");
    }
    // The authored identity line (design file 36:51). The Patch number is
    // projected by this surface's own summary; the name beside it is the main
    // surface's projected Patch name. `summary.capabilityId` used to stand
    // there — the capability's identity, not its name, which is the class of
    // value FR-014 keeps off the screen.
    var main = surfaceById(model, "patchMain");
    var mainSummary = (main && main.summary) || null;
    var empty = summary.kind === "emptyPatchUtility";
    var identity =
      '<span class="type-hint focus" data-role="patch-identity">' +
      escapeHtml(empty ? "NEW" : String(summary.patchId)) +
      HINT_SEPARATOR +
      escapeHtml(
        empty
          ? "DEFAULT · CAPACITY " + String(summary.activeCount) + "/" + String(summary.capacity)
          : mainSummary && mainSummary.kind === "patch"
          ? String(mainSummary.patchName)
          : UNAVAILABLE_MARK
      ) +
      "</span>";
    return (
      '<span class="type-label muted">' +
      escapeHtml(String(surface.label || "UTILITY")) +
      "</span>" +
      identity +
      rows
    );
  }

  function mixerIdentity(control, id) {
    if (control && control.label) {
      var label = String(control.label);
      var match = label.match(/^(T[0-9A-Fa-f]{2})\s+(.+)$/);
      return match
        ? match[1].toUpperCase() + " / " + match[2].toUpperCase()
        : label.toUpperCase();
    }
    return id && id.kind === "track"
      ? trackName(id.trackId) + " / " + String(id.parameter).toUpperCase()
      : UNAVAILABLE;
  }

  // The persistent MIXER Inspector: a correlation header bound to the
  // selected main-track control and a separately scrollable body composed
  // from the Inspector surface's canonical FocusPath-ordered controls.
  function mixerInspectorHtml(model, inspector) {
    var main = surfaceById(model, "mixerMain");
    var summary = inspector.summary || {};
    var trackId = focusedTrackId(model);
    var correlatedId = summary.focusedControl || null;
    var focused = controlById(main, JSON.stringify(correlatedId || null));
    var columns = trackColumns(main);
    var column = null;
    for (var i = 0; i < columns.length; i += 1) {
      if (columns[i].trackId === trackId) {
        column = columns[i];
      }
    }

    // Big readout: the focused control's value in its bound presentation
    // form — the focused level reads in the LevelReadout's MidiHexadecimal
    // binding; a toggle reads ON/OFF; any other continuous reads 0–127.
    var big = UNAVAILABLE;
    var bigTone = "muted";
    var rangeHint = "";
    if (focused) {
      var parameter =
        focused.path.controlId.id && focused.path.controlId.id.parameter;
      if (focused.kind === "toggle") {
        big = toggleOn(focused) ? "ON" : "OFF";
        bigTone = toggleOn(focused) ? "positive" : "secondary";
      } else if (parameter === "level") {
        big = midiHex(focused);
        bigTone = "positive";
        rangeHint = "LEVEL 00 — 7F";
      } else if (parameter === "pan") {
        big = panCondition(focused);
        bigTone = "positive";
        rangeHint = "PAN L — C — R";
      } else if (focused.numericRange) {
        big = midiDecimal(focused);
        bigTone = "positive";
        rangeHint = "0 — 127";
      }
    }

    var muteSolo = "";
    if (column && (column.mute || column.solo)) {
      var muteValue = column.mute
        ? toggleOn(column.mute)
          ? '<span class="warning">ON</span>'
          : "OFF"
        : UNAVAILABLE;
      var soloValue = column.solo
        ? toggleOn(column.solo)
          ? '<span class="positive">ON</span>'
          : "OFF"
        : UNAVAILABLE;
      muteSolo =
        '<div class="rule"></div>' +
        '<table class="type-hint secondary" data-role="mute-solo">' +
        "<tbody>" +
        '<tr><td>MUTE</td><td data-role="mute">' +
        muteValue +
        "</td></tr>" +
        '<tr><td>SOLO</td><td data-role="solo">' +
        soloValue +
        "</td></tr>" +
        "</tbody></table>";
    }

    var routes = summary.routedPatches || [];
    var routeRows = "";
    for (var p = 0; p < routes.length; p += 1) {
      routeRows +=
        '<li data-route="' +
        p +
        '" data-patch-id="' +
        escapeHtml(String(routes[p].patchId)) +
        '" data-patch-name="' +
        escapeHtml(String(routes[p].patchName)) +
        '"><span class="route-id">P' +
        String(routes[p].patchId).padStart(2, "0") +
        '</span><span class="route-name">' +
        escapeHtml(String(routes[p].patchName)) +
        "</span></li>";
    }
    var routing = routeRows
      ? '<ul class="route-list type-hint secondary" data-role="routes">' +
        routeRows +
        "</ul>"
      : '<span class="type-hint muted empty-route" data-role="routes">EMPTY</span>';

    var controlRows = "";
    var controls = inspector.controls || [];
    for (var c = 0; c < controls.length; c += 1) {
      if (controls[c].visible) {
        controlRows += patchRowHtml(controls[c], model.interactionMode, "panel");
      }
    }

    return (
      '<div class="inspector-pinned" data-role="inspector-correlation">' +
      '<span class="type-label muted">CURSOR</span>' +
      '<span class="type-value focus" data-role="cursor">' +
      escapeHtml(mixerIdentity(focused, correlatedId)) +
      "</span>" +
      '<div class="inspector-reading"><span class="type-display big-readout ' +
      bigTone +
      '" data-role="big-readout">' +
      escapeHtml(big) +
      "</span>" +
      '<span class="type-value secondary inspector-meter" id="inspector-meter-readout" data-meter-state="stale">RMS — / PEAK — dBFS</span></div>' +
      (rangeHint
        ? '<span class="type-hint muted" data-role="readout-range">' +
          escapeHtml(rangeHint) +
          "</span>"
        : "") +
      muteSolo +
      '<div class="rule"></div><span class="type-label muted">ROUTING</span>' +
      routing +
      "</div>" +
      '<div class="inspector-controls" data-role="inspector-controls">' +
      controlRows +
      "</div>"
    );
  }

  function footerHtml(model) {
    var breadcrumb = settingsSurface(model)
      ? "SETTINGS"
      : surfaceById(model, "sends") ? "SENDS" : String(model.context || "").toUpperCase();
    if (hasWaveform(surfaceByRole(model, "detail")) && !surfaceByRole(model, "modal")) {
      var origin = controlByPath(surfaceById(model, "patchMain"), model.returnPath && model.returnPath.origin);
      breadcrumb += " / DETAIL / " + (origin ? controlValueText(origin) : UNAVAILABLE_MARK);
      if (model.activeSurface === "patchUtility") breadcrumb += " / UTILITY";
    } else {
      var identity = focusIdentity(model);
      if (identity) breadcrumb += " / " + identity;
    }
    return (
      '<span class="type-hint secondary" data-role="breadcrumb">' +
      escapeHtml(breadcrumb) +
      "</span>" +
      '<span class="spring"></span>' +
      '<span class="footer-guidance" data-role="footer-guidance">' +
      hintRun(model.validActions || []) +
      "</span>"
    );
  }

  // Host workspaces reuse the same shell, row component, tokens and input path.
  // All navigation, selection and edits stay in the Rust owner.
  var activeWorkspace = null;
  var hostView = null;

  function workspaceHints(hints) {
    return '<span class="workspace-hints" data-role="footer-guidance" aria-label="Current controls">' +
      String(hints).split('\n').map(function (line) {
        return '<span class="workspace-hint-line">' + line.split(' · ').map(function (hint) {
          return '<span class="workspace-hint">' + escapeHtml(hint) + '</span>';
        }).join(' · ') + '</span>';
      }).join(' ') + '</span>';
  }

  function workspaceRows(rows, workspace) {
    var keyIds = workspace.preview ? workspace.preview.keys.map(function (key) { return key.id; }) : [];
    var detailIds = workspace.preview ? (workspace.preview.controls || []).map(function (row) { return row.id; }) : [];
    return workspaceRowButtons(rows.filter(function (row) { return keyIds.indexOf(row.id) < 0 && detailIds.indexOf(row.id) < 0; }), workspace);
  }

  function workspaceRowButtons(rows, workspace) {
    return rows.map(function (row) {
      var focused = workspace.focus === row.id;
      return '<button class="workspace-action" ' + (row.enabled === false ? 'disabled ' : '') + 'data-span="' + row.span + '" data-selected="' + row.selected + '" data-workspace-id="' + escapeHtml(row.id) +
        '" aria-current="' + String(focused) + '"><span class="workspace-focus-marker">' + (focused ? '›' : '') +
        '</span><span class="workspace-action-label">' + (row.selected ? '✓ ' : '') + escapeHtml(row.label) + '</span><span class="workspace-action-value">' + escapeHtml(row.value) + '</span></button>';
    }).join("");
  }

  function workspaceToolbar(rows, timeline) {
    return rows.length ? '<nav class="workspace-toolbar" data-timeline="' + !!timeline + '">' + rows.map(function (row) {
      return '<button data-workspace-id="' + escapeHtml(row.id) + '">' + '<span>' + escapeHtml(row.label) + '</span>' + (timeline ? '<strong>' + escapeHtml(row.value) + '</strong>' : '') + '</button>';
    }).join('') + '</nav>' : '';
  }

  function workspacePreview(workspace) {
    var preview = workspace.preview;
    if (!preview) return '';
    var keys = preview.keys.map(function (key) {
      var interval = Number(key.value);
      var black = [1, 3, 6, 8, 10].indexOf((interval % 12 + 12) % 12) >= 0;
      return '<button class="workspace-piano-key" data-black="' + black + '" data-selected="' + key.selected + '" aria-pressed="' + key.selected +
        '" aria-current="' + (workspace.focus === key.id) + '" data-workspace-id="' + escapeHtml(key.id) + '">' + escapeHtml(key.label) + '</button>';
    }).join('');
    return '<section class="workspace-preview"><h2>' + escapeHtml(preview.title) + '</h2><p class="type-hint secondary">' + escapeHtml(preview.caption) +
      '</p><div class="workspace-intervals">' + preview.values.map(function (value) { return '<span>' + escapeHtml(value) + '</span>'; }).join('') +
      '</div><p class="workspace-resolved">' + escapeHtml(preview.footer) + '</p>' +
      (keys ? '<div class="workspace-keyboard" aria-label="Chord interval keyboard">' + keys + '</div>' : '') +
      '<div class="workspace-preview-controls">' + workspaceRowButtons(preview.controls || [], workspace) + '</div></section>';
  }

  function workspaceSpan(span, original) {
    return '<button class="workspace-duration" data-original="' + original + '" ' + (original ? 'tabindex="-1" aria-hidden="true" ' : '') +
      'data-workspace-id="' + escapeHtml(span.id) + '" data-start="' + span.start + '" data-end="' + span.end +
      '" data-selected="' + span.selected + '" aria-pressed="' + span.selected + '">' + (original ? '' : escapeHtml(span.label)) + '</button>';
  }

  function workspaceTimeLane(workspace, row, lane) {
    var timeline = workspace.timeline;
    var cells = row.cells.map(function (cell, index) {
      return '<button class="workspace-time-cell" data-beat="' + (timeline.beatColumns.indexOf(index) >= 0) + '" data-workspace-id="' + escapeHtml(cell.id) +
        '" aria-label="' + escapeHtml(row.label + ', ' + cell.label) + '"></button>';
    }).join('');
    var originals = timeline.originals.filter(function (span) { return span.lane === lane; }).map(function (span) { return workspaceSpan(span, true); }).join('');
    var spans = timeline.spans.filter(function (span) { return span.lane === lane; }).map(function (span) { return workspaceSpan(span, false); }).join('');
    var cursor = lane === timeline.cursorLane && timeline.cursor !== null ? '<i class="workspace-cursor-cell" data-start="' + timeline.cursor + '" data-end="' + Math.min(10000, timeline.cursor + timeline.cursorWidth) + '"></i>' : '';
    return '<div class="workspace-time-lane" data-secondary="' + (timeline.secondaryLanes.indexOf(lane) >= 0) + '" data-muted="' + (timeline.mutedLanes.indexOf(lane) >= 0) + '"><span class="workspace-pitch-label">' + escapeHtml(row.label) +
      '</span><div class="workspace-time-track"><div class="workspace-time-cells">' + cells + '</div>' + originals + spans + cursor + '</div></div>';
  }

  function workspaceCurve(curve) {
    var path = curve.path.map(function (point) { return point[0] + ',' + (10000 - point[1]); }).join(' ');
    var points = curve.points.map(function (point) {
      return '<span class="workspace-curve-point" role="img" aria-label="' + escapeHtml(point.label) + '" title="' + escapeHtml(point.label) +
        '" data-start="' + point.position + '" data-value="' + point.value + '" data-selected="' + point.selected + '"></span>';
    }).join('');
    return '<section class="workspace-curve" aria-label="' + escapeHtml(curve.label) + '"><div class="workspace-curve-heading"><strong>' + escapeHtml(curve.label) +
      '</strong><span>' + escapeHtml(curve.caption) + '</span></div><div class="workspace-curve-body"><div class="workspace-curve-scale"><span>' + curve.maximum +
      '</span><span>0</span></div><div class="workspace-curve-track"><svg viewBox="0 0 10000 10000" preserveAspectRatio="none" aria-hidden="true"><polyline points="' + path +
      '"></polyline></svg>' + points + (curve.path.length ? '' : '<span class="workspace-curve-empty">No points in this window</span>') + '</div></div></section>';
  }

  function workspaceTimeline(workspace) {
    var timeline = workspace.timeline;
    var headings = workspace.columns.map(function (column) { return '<span>' + escapeHtml(column) + '</span>'; }).join('');
    var lanes = workspace.grid.map(function (row, lane) { return workspaceTimeLane(workspace, row, lane); }).join('');
    var levels = timeline.spans.map(function (span) {
      return '<button class="workspace-level" aria-label="' + escapeHtml(span.label) + ' velocity ' + span.level + '" data-workspace-id="' + escapeHtml(span.levelId) + '" data-start="' + span.start +
        '" data-level="' + span.level + '" data-selected="' + span.selected + '"></button>';
    }).join('');
    var beatLines = timeline.beatColumns.map(function (index) { return '<i class="workspace-beat-line" data-start="' + Math.round(index / workspace.columns.length * 10000) + '"></i>'; }).join('');
    return '<div class="workspace-timeline" data-columns="' + workspace.columns.length + '"><div class="workspace-time-ruler"><span></span><div class="workspace-time-cells">' + headings +
      '</div></div><div class="workspace-time-body">' + lanes +
      (timeline.cursor !== null ? '<div class="workspace-playhead-track"><i class="workspace-edit-cursor" aria-label="Edit cursor" data-start="' + timeline.cursor + '"></i></div>' : '') +
      (timeline.playhead !== null ? '<div class="workspace-playhead-track"><i class="workspace-playhead" aria-label="Playback position" data-start="' + timeline.playhead + '"></i></div>' : '') +
      '</div>' + (timeline.curve ? workspaceCurve(timeline.curve) : '<div class="workspace-velocity" data-focused="' + timeline.levelFocused + '"><button class="workspace-velocity-label" data-workspace-id="' + escapeHtml(timeline.levelAction.id) + '">' + escapeHtml(timeline.levelAction.label) + '<span>127</span><span>1</span></button><div class="workspace-levels">' + beatLines + levels + '</div></div>') + '</div>';
  }

  function layoutWorkspace(doc, workspace) {
    doc.querySelectorAll('.workspace-popover').forEach(function (element) { element.style.setProperty('--panel-columns', element.dataset.columns || 1); });
    doc.querySelectorAll('.workspace-popover .workspace-action').forEach(function (element) { element.style.gridColumn = 'span ' + (element.dataset.span || 1); });
    doc.querySelectorAll('.workspace-timeline').forEach(function (element) { element.style.setProperty('--time-columns', element.dataset.columns); });
    doc.querySelectorAll('.workspace-duration, .workspace-level, .workspace-playhead, .workspace-edit-cursor, .workspace-cursor-cell, .workspace-beat-line, .workspace-curve-point').forEach(function (element) {
      element.style.left = (Number(element.dataset.start) / 100) + '%';
      if (element.dataset.end) element.style.width = Math.max(0.25, (Number(element.dataset.end) - Number(element.dataset.start)) / 100) + '%';
      if (element.dataset.value) element.style.bottom = (Number(element.dataset.value) / 100) + '%';
      if (element.dataset.level) element.style.height = (Number(element.dataset.level) / 127 * 100) + '%';
    });
  }

  function workspaceGridRow(row, workspace) {
    var cells = row.cells.map(function (cell) {
      return '<td><button class="workspace-cell type-value" data-workspace-id="' + escapeHtml(cell.id) +
        '" data-focused="' + String(workspace.focus === cell.id) + '" data-selected="' + String(cell.selected) + '" data-playing="' + String(cell.playing) + '" aria-current="' +
        String(workspace.focus === cell.id) + '">' + escapeHtml(cell.value) + '</button></td>';
    }).join("");
    return '<tr data-playing="' + String(row.playing) + '"><th scope="row">' +
      (row.playing ? '▶ ' : '') + escapeHtml(row.label) + '</th>' + cells + '</tr>';
  }

  function workspaceGrid(workspace) {
    if (workspace.timeline) return workspaceTimeline(workspace);
    if (!workspace.columns.length) return "";
    var header = '<th scope="col">ROW</th>' + workspace.columns.map(function (column) {
      return '<th scope="col">' + escapeHtml(column) + '</th>';
    }).join("");
    var rows = workspace.grid.map(function (row) {
      return workspaceGridRow(row, workspace);
    }).join("");
    return '<div class="workspace-grid-scroll"><table class="workspace-grid" data-compact="' + String(workspace.compactGrid) + '"><thead><tr>' + header +
      '</tr></thead><tbody>' + rows + '</tbody></table></div>';
  }

  function emitWorkspace(event) {
    if (activeWorkspace && activeWorkspace.busy) return;
    if (window.__TAURI__) window.__TAURI__.event.emit("crest://host-input", event).catch(function (error) { emitRenderError(error, null); });
  }

  var lastWorkspacePageSize = null;
  function reportWorkspacePageSize(workspace) {
    var list = window.document.querySelector('.workspace-rows');
    var row = list && list.querySelector('[data-workspace-id]');
    if (!row || !workspace.rows.length || list.clientHeight <= 0) {
      lastWorkspacePageSize = null;
      return;
    }
    var height = row.getBoundingClientRect().height;
    if (height <= 0) return;
    var observation = {
      kind: 'listPageSize',
      id: workspace.rows[0].id,
      rows: Math.max(1, Math.min(workspace.rows.length, Math.floor(list.clientHeight / height)))
    };
    var key = JSON.stringify(observation);
    if (key !== lastWorkspacePageSize) {
      lastWorkspacePageSize = key;
      emitWorkspace(observation);
    }
  }

  function attachWorkspaceInput() {
    window.document.querySelectorAll("[data-workspace-id]").forEach(function (element) {
      element.onclick = function () { emitWorkspace({kind: "activate", id: element.dataset.workspaceId}); };
    });
    var form = window.document.getElementById("workspace-text-form");
    if (form) {
      form.elements.value.oninput = function () { emitWorkspace({kind: "textChanged", id: form.dataset.id, value: form.elements.value.value}); };
      form.onsubmit = function (event) {
        event.preventDefault();
        emitWorkspace({kind: "submit", id: form.dataset.id, value: form.elements.value.value});
      };
    }
  }

  function appendWorkspaceEntry() {
    if (!hostView) return;
    var context = window.document.getElementById("context-line");
    context.insertAdjacentHTML("beforeend", '<button class="workspace-home type-label" data-workspace-id="' + escapeHtml(hostView.entry.id) + '">' + escapeHtml(hostView.entry.label) + '</button>');
    attachWorkspaceInput();
  }

  function renderWorkspace(workspace) {
    var doc = window.document;
    var previous = doc.getElementById("workspace-text-form");
    var draft = previous ? {id: previous.dataset.id, value: previous.elements.value.value, baseline: previous.dataset.value, start: previous.elements.value.selectionStart} : null;
    doc.body.classList.remove("settings-active", "sends-active");
    doc.body.classList.add("workspace-active");
    doc.body.dataset.workspaceContext = workspace.context;
    doc.body.dataset.workspaceTimeline = !!workspace.timeline;
    doc.getElementById("context-line").innerHTML = '<span class="type-label">' + escapeHtml(hostView.name) + '</span><span class="spring"></span><button class="workspace-tab" data-current="true" data-workspace-id="home">SONG</button><button class="workspace-tab" data-workspace-id="instruments">INSTRUMENTS</button><button class="workspace-tab" data-workspace-id="menu">MENU</button><button class="workspace-transport" data-workspace-id="transport" aria-label="Play or stop">' + (workspace.playing ? '■ STOP' : '▶ PLAY') + '</button>';
    doc.getElementById("identity-header").innerHTML = '<span class="workspace-context type-label">' + escapeHtml(workspace.timeline ? workspace.timeline.label : workspace.context) + '</span><span class="type-heading">' + escapeHtml(workspace.title) + '</span><span class="spring"></span><span class="type-hint secondary">' + escapeHtml(workspace.subtitle) + '</span>';
    if (workspace.timeline) doc.getElementById("identity-header").insertAdjacentHTML("beforeend", '<div class="workspace-roll-meta"><span>' + escapeHtml(workspace.timeline.caption) + '</span><span>' + escapeHtml(workspace.timeline.viewLabel) + '</span></div>');
    var list = '<div class="workspace-list-preview"><div class="workspace-rows">' + workspaceRows(workspace.rows, workspace) + '</div>' + workspacePreview(workspace) + '</div>';
    var grid = workspaceGrid(workspace);
    var tabs = '<nav class="workspace-tabs" aria-label="Settings pages">' + (workspace.tabs || []).map(function (tab) {
      return '<button class="workspace-tab" data-workspace-id="' + escapeHtml(tab.id) + '" data-focused="' + (workspace.focus === tab.id) + '" data-current="' + tab.selected + '" aria-current="' + tab.selected + '">' + escapeHtml(tab.label) + '</button>';
    }).join('') + '</nav>';
    var body = tabs + grid + list + workspaceToolbar(workspace.inspector, workspace.timeline);
    if (workspace.overlay) {
      body = '<div class="workspace-backdrop" inert>' + grid + workspaceToolbar(workspace.inspector, workspace.timeline) + '</div><section class="workspace-popover" data-columns="' + workspace.panelColumns + '" data-timeline="' + !!workspace.timeline + '" role="dialog" aria-modal="true" aria-label="' + escapeHtml(workspace.context) + '"><h1 class="type-heading">' + escapeHtml(workspace.context) + '</h1>' + (workspace.panelCaption ? '<p class="workspace-panel-caption">' + escapeHtml(workspace.panelCaption) + '</p>' : '') + list + '</section>';
    }
    if (workspace.busy) {
      body = '<div class="workspace-backdrop" inert aria-hidden="true">' + grid + '</div><section class="workspace-popover" data-busy="true" role="dialog" aria-modal="true" aria-labelledby="workspace-loading-title" tabindex="-1"><h1 id="workspace-loading-title" class="type-heading">' + escapeHtml(workspace.context) + '</h1><div class="workspace-loading-spinner" aria-hidden="true"></div><p class="workspace-panel-caption" role="status" aria-live="polite">' + escapeHtml(workspace.panelCaption) + '</p></section>';
    }
    if (workspace.textEntry) {
      var entry = workspace.textEntry;
      var value = draft && draft.id === entry.id && draft.baseline === entry.value ? draft.value : entry.value;
      body = '<form id="workspace-text-form" data-value="' + escapeHtml(entry.value) + '" data-id="' + escapeHtml(entry.id) + '"><label class="type-label">' + escapeHtml(entry.label) + '<input name="value" aria-label="' + escapeHtml(entry.label) + '" value="' + escapeHtml(value) + '" /></label><button type="submit">' + escapeHtml(entry.submitLabel) + '</button></form>' + body;
    }
    doc.getElementById("workspace").innerHTML = '<div class="workspace-page">' + body + '</div>';
    doc.getElementById("inspector").innerHTML = '';
    doc.getElementById("footer").innerHTML = (workspace.summary ? '<div class="workspace-summary">' + escapeHtml(workspace.summary).replace(/\n/g, '<br>') + '<small>' + escapeHtml(workspace.status) + '</small></div>' : '<span class="type-hint workspace-status">' + escapeHtml(workspace.status) + '</span>') + workspaceHints(workspace.hints);
    layoutWorkspace(doc, workspace);
    attachWorkspaceInput();
    if (workspace.busy) {
      doc.querySelectorAll('#context-line button, [data-workspace-id]').forEach(function (element) {
        element.disabled = true;
        element.setAttribute('aria-disabled', 'true');
      });
      doc.querySelector('.workspace-popover[data-busy="true"]').focus({preventScroll: true});
    }
    applyDynamicGeometry(doc);
    var focused = doc.querySelector('[data-workspace-id="' + CSS.escape(workspace.focus) + '"]');
    if (focused) focused.scrollIntoView({block: "nearest", inline: "nearest"});
    reportWorkspacePageSize(workspace);
    var input = doc.querySelector('#workspace-text-form input');
    if (input) { input.focus({preventScroll: true}); if (draft && draft.start !== null) input.setSelectionRange(draft.start, draft.start); }
  }

  function observeWorkspace(view) {
    hostView = view;
    activeWorkspace = view.workspace;
    if (activeWorkspace) renderWorkspace(activeWorkspace);
    else {
      lastWorkspacePageSize = null;
      if (latestModel) render(latestModel);
    }
  }


  // ---- the pure render ---------------------------------------------------

  // CSP: the production policy (style-src 'self', no unsafe-inline) blocks
  // every parsed inline style attribute, but CSSOM property assignment is
  // exempt by spec — dynamic geometry must go through here. The rendered
  // HTML carries the value as a data attribute (data-level / data-position);
  // this one pass reads those attributes back and applies the custom
  // properties page.css consumes. A pure function of the DOM — no state, no
  // memory of prior renders — so the same document still paints the same
  // geometry (determinism proof target). Absence of the attribute means "no
  // data", never "zero": unavailable branches emit no attribute and get no
  // property.
  function applyDynamicGeometry(doc) {
    var levels = doc.querySelectorAll("[data-level]");
    for (var i = 0; i < levels.length; i += 1) {
      levels[i].style.setProperty(
        "--level",
        levels[i].getAttribute("data-level")
      );
    }
    var positions = doc.querySelectorAll("[data-position]");
    for (var j = 0; j < positions.length; j += 1) {
      positions[j].style.setProperty(
        "--position",
        positions[j].getAttribute("data-position")
      );
    }
    var waveformValues = ["wave-top", "wave-height", "landmark-position"];
    for (var field = 0; field < waveformValues.length; field += 1) {
      var name = waveformValues[field];
      var nodes = doc.querySelectorAll("[data-" + name + "]");
      for (var node = 0; node < nodes.length; node += 1) {
        nodes[node].style.setProperty("--" + name, nodes[node].getAttribute("data-" + name));
      }
    }
  }

  // Focus visibility follows the serialized semantic path. The DOM is
  // searched by identity after every paint/reflow; no row number becomes
  // application state and scrolling dispatches no semantic event.
  function revealSemanticFocus(doc, model) {
    if (!model) {
      return;
    }
    var wanted = JSON.stringify(model.focusPath || null);
    var rows = doc.querySelectorAll('[data-focus-path]');
    for (var i = 0; i < rows.length; i += 1) {
      if (rows[i].getAttribute("data-focus-path") === wanted) {
        rows[i].scrollIntoView({ block: "nearest", inline: "nearest" });
        return;
      }
    }
  }

  // Rebuilds the five shell bands from one deserialized
  // SemanticGraphicalViewModel document. Same document, identical DOM. The
  // context line, identity header, workspace scaffold, side region, and
  // footer are the same shared bands for both contexts; only the workspace
  // body and the side reading follow the document's own context.
  function render(model) {
    if (activeWorkspace) { renderWorkspace(activeWorkspace); return; }
    window.document.body.classList.remove("workspace-active");
    var doc = window.document;
    var priorInput = doc.getElementById("send-name-input");
    var priorForm = doc.getElementById("send-name-form");
    var nameDraft = priorInput && priorForm ? { bus: priorForm.dataset.bus, value: priorInput.value, start: priorInput.selectionStart, end: priorInput.selectionEnd } : null;
    latestModel = model;
    var main = surfaceById(model, "mixerMain");
    var columns = trackColumns(main);
    var settings = settingsSurface(model);
    var sends = surfaceById(model, "sends");
    doc.body.classList.toggle("settings-active", Boolean(settings));
    doc.body.classList.toggle("sends-active", Boolean(sends));
    doc.getElementById("context-line").innerHTML = contextLineHtml(model);
    doc.getElementById("identity-header").innerHTML = identityHeaderHtml(
      model,
      columns
    );
    doc.getElementById("workspace").innerHTML =
      settings
        ? settings.id === "saveLoadSettings"
          ? saveLoadSettingsWorkspaceHtml(model)
          : settings.id === "controllerSettings"
          ? controllerSettingsWorkspaceHtml(model)
          : midiSettingsWorkspaceHtml(model)
        : sends ? sendsWorkspaceHtml(model)
        : model.context === "patch"
        ? patchWorkspaceHtml(model)
        : mixerWorkspaceHtml(model, columns);
    doc.getElementById("inspector").innerHTML = settings
      ? settings.id === "saveLoadSettings"
        ? saveLoadSettingsInspectorHtml()
        : settings.id === "controllerSettings"
        ? controllerSettingsInspectorHtml(model)
        : midiInputInspectorHtml(model)
      : sends ? '<span class="type-label muted">SEND CHAIN</span><p class="type-value">' + escapeHtml(sends.summary.name) + '</p><p class="type-hint muted">Shared effects for Patch sends</p>' : sideRegionHtml(model);
    doc.getElementById("footer").innerHTML = footerHtml(model);
    // Final step, after ALL five region insertions: apply the dynamic
    // geometry in the same paint, on the initial render and every re-render
    // alike. render() is the single place projection content enters the DOM.
    applyDynamicGeometry(doc);
    revealSemanticFocus(doc, model);
    attachSendNameInput(nameDraft);
    appendWorkspaceEntry();
  }

  // ---- the structural observation (acceptance harness contract) -----------

  // Renders the document, then reads the painted DOM back into post-paint
  // structural evidence. Everything in the returned object is copied from
  // what was actually painted — not from the input document — so a plan that
  // never reached the screen cannot satisfy it. The MIXER fields (columns,
  // inspector, meter) and the PATCH fields (rows, utility, annotations) are
  // both always present; whichever context did not paint reports its empty
  // shape, deterministically.
  //
  // Two calls with one document at one window size return deep-equal
  // objects; inspector.widthPx is asserted >= 320 at the compact viewport.
  function renderObservation(model) {
    render(model);
    var doc = window.document;

    function painted(id) {
      var el = doc.getElementById(id);
      if (!el) {
        return false;
      }
      var rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    }

    function textOf(root, selector) {
      var el = root.querySelector(selector);
      return el ? el.textContent.replace(/\s+/g, " ").trim() : null;
    }

    function rectOf(element) {
      if (!element) {
        return null;
      }
      var rect = element.getBoundingClientRect();
      return {
        xPx: Math.round(rect.x),
        yPx: Math.round(rect.y),
        widthPx: Math.round(rect.width),
        heightPx: Math.round(rect.height),
        rightPx: Math.round(rect.right),
        bottomPx: Math.round(rect.bottom),
      };
    }

    function fullyVisible(element) {
      if (!element) {
        return false;
      }
      var rect = element.getBoundingClientRect();
      return (
        rect.top >= -1 &&
        rect.left >= -1 &&
        rect.bottom <= window.innerHeight + 1 &&
        rect.right <= window.innerWidth + 1
      );
    }

    // Exercise both ends of an independently scrollable region and restore
    // its presentation state before returning the observation. Merely
    // comparing scrollHeight and clientHeight proves that overflow exists;
    // it does not prove the browser can actually reach all of it.
    function scrollReachability(element) {
      if (!element) {
        return null;
      }
      var originalTop = element.scrollTop;
      var maximumTop = Math.max(0, element.scrollHeight - element.clientHeight);
      var semanticTargets = element.querySelectorAll("[data-focus-path]");
      var firstTarget = semanticTargets.length > 0 ? semanticTargets[0] : null;
      var lastTarget =
        semanticTargets.length > 0
          ? semanticTargets[semanticTargets.length - 1]
          : null;
      element.scrollTop = 0;
      var startReachable = Math.abs(element.scrollTop) <= 1;
      var containerRect = element.getBoundingClientRect();
      // Headings can precede the first control and passive visualizations can
      // follow the last. Reach each target using this region's actual scroll
      // range; a control need not coincide with an absolute content endpoint.
      function targetReachable(target) {
        if (!target) {
          return true;
        }
        var rect = target.getBoundingClientRect();
        element.scrollTop += rect.top - containerRect.top;
        rect = target.getBoundingClientRect();
        return (
          rect.top >= containerRect.top - 1 &&
          rect.bottom <= containerRect.bottom + 1
        );
      }
      var firstTargetReachable = targetReachable(firstTarget);
      element.scrollTop = maximumTop;
      var endReachable = Math.abs(element.scrollTop - maximumTop) <= 1;
      var lastTargetReachable = targetReachable(lastTarget);
      element.scrollTop = originalTop;
      return {
        scrollableBy: maximumTop,
        startReachable: startReachable,
        endReachable: endReachable,
        firstTargetReachable: firstTargetReachable,
        lastTargetReachable: lastTargetReachable,
      };
    }

    var bands = {
      contextLine: painted("context-line"),
      identityHeader: painted("identity-header"),
      workspace: painted("workspace"),
      inspector: painted("inspector"),
      footer: painted("footer"),
    };

    var computedRoot = window.getComputedStyle(doc.documentElement);
    var layoutMode = computedRoot
      .getPropertyValue("--layout-mode")
      .trim()
      .replace(/^['\"]|['\"]$/g, "");
    var regionElements = [
      ["contextLine", doc.getElementById("context-line")],
      ["identityHeader", doc.getElementById("identity-header")],
      ["workspace", doc.getElementById("workspace")],
      ["inspector", doc.getElementById("inspector")],
      ["footer", doc.getElementById("footer")],
    ];
    var regions = [];
    for (var regionIndex = 0; regionIndex < regionElements.length; regionIndex += 1) {
      regions.push({
        id: regionElements[regionIndex][0],
        bounds: rectOf(regionElements[regionIndex][1]),
      });
    }
    var visualRegionOrder = regions
      .slice()
      .sort(function (left, right) {
        return (
          left.bounds.yPx - right.bounds.yPx ||
          left.bounds.xPx - right.bounds.xPx
        );
      })
      .map(function (region) {
        return region.id;
      });

    var columns = [];
    var columnNodes = doc.querySelectorAll("#bank .column");
    for (var i = 0; i < columnNodes.length; i += 1) {
      var node = columnNodes[i];
      var structures = [];
      var structureNodes = node.querySelectorAll("[data-structure]");
      for (var s = 0; s < structureNodes.length; s += 1) {
        structures.push(structureNodes[s].getAttribute("data-structure"));
      }
      var columnMeter = node.querySelector("[data-meter-track]");
      columns.push({
        trackId: Number(node.getAttribute("data-track")),
        header: textOf(node, '[data-structure="TrackHeader"]'),
        structures: structures,
        focused: node.classList.contains("focused"),
        correlated: node.classList.contains("correlated"),
        focusedRow: node.getAttribute("data-focused-row") || null,
        levelHex: textOf(node, '[data-structure="LevelReadout"]'),
        pan: textOf(node, '[data-structure="PanReadout"]'),
        stateLine: textOf(node, '[data-structure="StateLine"]'),
        rowStates: Array.prototype.map.call(
          node.querySelectorAll("[data-control-row]"),
          function (row) {
            return {
              row: row.getAttribute("data-control-row"),
              state: row.getAttribute("data-row-state"),
            };
          }
        ),
        meterState: columnMeter
          ? columnMeter.getAttribute("data-meter-state")
          : null,
        meterLabel: columnMeter
          ? columnMeter.getAttribute("aria-label")
          : null,
      });
    }

    // The painted width of one element inside a row, or null when the row does
    // not carry it. Rounded, so two renders of one document at one window size
    // report the identical integer.
    function widthOf(root, selector) {
      var el = root.querySelector(selector);
      return el ? Math.round(el.getBoundingClientRect().width) : null;
    }

    // The painted top and bottom edges of one element, in the row's own
    // coordinates — enough to see which of a wrapping row's lines it landed
    // on, which is the fact a width alone cannot report.
    function edgesIn(row, selector) {
      var el = row.querySelector(selector);
      if (!el) {
        return null;
      }
      var box = el.getBoundingClientRect();
      var origin = row.getBoundingClientRect().top;
      return {
        topPx: Math.round(box.top - origin),
        bottomPx: Math.round(box.bottom - origin),
      };
    }

    // Shared subordinate/Utility rows, in painted order, with measured target
    // geometry. Overview controls have their own section-aware report below.
    function rowReport(nodes) {
      var out = [];
      for (var r = 0; r < nodes.length; r += 1) {
        var rowNode = nodes[r];
        out.push({
          control: rowNode.getAttribute("data-control"),
          focusPath: rowNode.getAttribute("data-focus-path"),
          state: rowNode.getAttribute("data-state"),
          focusTreatment: rowNode.getAttribute("data-focus-treatment"),
          enabled: rowNode.getAttribute("data-enabled") === "true",
          visible: rowNode.getAttribute("data-visible") === "true",
          editable: rowNode.getAttribute("data-editable") === "true",
          validActionCount: Number(
            rowNode.getAttribute("data-valid-action-count") || 0
          ),
          label: textOf(rowNode, ".prow-label"),
          index: textOf(rowNode, ".prow-index"),
          value: textOf(rowNode, ".prow-value"),
          mark: textOf(rowNode, '[data-role="state-mark"]'),
          unit: textOf(rowNode, ".prow-unit"),
          range: textOf(rowNode, '[data-role="row-range"]'),
          hints: textOf(rowNode, '[data-role="row-hints"]'),
          interaction: rowNode.getAttribute("data-interaction"),
          readOnly: textOf(rowNode, '[data-role="read-only"]'),
          browse: textOf(rowNode, '[data-role="browse"]'),
          position: rowNode.querySelector("[data-position]")
            ? Number(
                rowNode
                  .querySelector("[data-position]")
                  .getAttribute("data-position")
              )
            : null,
          heightPx: Math.round(rowNode.getBoundingClientRect().height),
          railPx: widthOf(rowNode, ".prow-position"),
          hintsPx: widthOf(rowNode, '[data-role="row-hints"]'),
          labelEdges: edgesIn(rowNode, ".prow-label"),
          hintEdges: edgesIn(rowNode, '[data-role="row-hints"]'),
        });
      }
      return out;
    }
    var rows = [];

    var overviewNode = doc.getElementById("patch-overview");
    var overview = null;
    if (overviewNode) {
      var overviewSections = [];
      var overviewSectionNodes = overviewNode.querySelectorAll(
        "[data-overview-section]"
      );
      for (var overviewSectionIndex = 0; overviewSectionIndex < overviewSectionNodes.length; overviewSectionIndex += 1) {
        var overviewSectionNode = overviewSectionNodes[overviewSectionIndex];
        var overviewControls = [];
        var overviewControlNodes = overviewSectionNode.querySelectorAll(
          ".overview-control"
        );
        for (var overviewControlIndex = 0; overviewControlIndex < overviewControlNodes.length; overviewControlIndex += 1) {
          var overviewControlNode = overviewControlNodes[overviewControlIndex];
          overviewControls.push({
            control: overviewControlNode.getAttribute("data-control"),
            focusPath: overviewControlNode.getAttribute("data-focus-path"),
            state: overviewControlNode.getAttribute("data-state"),
            focusTreatment: overviewControlNode.getAttribute(
              "data-focus-treatment"
            ),
            label: textOf(overviewControlNode, ".overview-control-label .type-label"),
            value: textOf(overviewControlNode, ".overview-control-value"),
            mark: textOf(overviewControlNode, '[data-role="state-mark"]'),
            parameterSummary: textOf(
              overviewControlNode,
              '[data-role="parameter-summary"]'
            ),
            hints: textOf(overviewControlNode, '[data-role="overview-hints"]'),
            bounds: rectOf(overviewControlNode),
            visible: fullyVisible(overviewControlNode),
          });
        }
        overviewSections.push({
          id: overviewSectionNode.getAttribute("data-overview-section"),
          label: textOf(overviewSectionNode, ".overview-section-heading h2"),
          bounds: rectOf(overviewSectionNode),
          controls: overviewControls,
        });
      }
      overview = {
        patchName: textOf(overviewNode, '[data-role="overview-patch-name"]'),
        bounds: rectOf(overviewNode),
        sections: overviewSections,
      };
    }

    // The painted detail composition, or null when no detail entry is open.
    var detailNode = doc.getElementById("detail");
    var detail = null;
    if (detailNode) {
      function maximumPaintedBottom(element) {
        var bottom = element.getBoundingClientRect().bottom;
        var descendants = element.querySelectorAll("*");
        for (var descendantIndex = 0; descendantIndex < descendants.length; descendantIndex += 1) {
          var descendantRect = descendants[descendantIndex].getBoundingClientRect();
          if (descendantRect.width > 0 && descendantRect.height > 0) {
            bottom = Math.max(bottom, descendantRect.bottom);
          }
        }
        return bottom;
      }

      function directChildOverlap(element) {
        var children = [];
        for (var childIndex = 0; childIndex < element.children.length; childIndex += 1) {
          var child = element.children[childIndex];
          var childRect = child.getBoundingClientRect();
          if (childRect.width > 0 && childRect.height > 0) {
            children.push(child);
          }
        }
        var overlap = 0;
        for (var adjacentIndex = 0; adjacentIndex + 1 < children.length; adjacentIndex += 1) {
          overlap = Math.max(
            overlap,
            maximumPaintedBottom(children[adjacentIndex]) -
              children[adjacentIndex + 1].getBoundingClientRect().top
          );
        }
        return Math.max(0, Math.round(overlap));
      }

      var detailSections = [];
      var detailSectionNodes = detailNode.querySelectorAll(
        "[data-detail-section]"
      );
      for (
        var detailSectionIndex = 0;
        detailSectionIndex < detailSectionNodes.length;
        detailSectionIndex += 1
      ) {
        var detailSectionNode = detailSectionNodes[detailSectionIndex];
        detailSections.push({
          id: detailSectionNode.getAttribute("data-detail-section"),
          label: textOf(
            detailSectionNode,
            '[data-role="detail-section-label"]'
          ),
          controlCount: detailSectionNode.querySelectorAll(
            ".prow[data-focus-path]"
          ).length,
          bounds: rectOf(detailSectionNode),
        });
      }
      var detailVisualizations = [];
      var detailVisualizationNodes = detailNode.querySelectorAll(
        "[data-visualization]"
      );
      for (
        var detailVisualizationIndex = 0;
        detailVisualizationIndex < detailVisualizationNodes.length;
        detailVisualizationIndex += 1
      ) {
        var detailVisualizationNode =
          detailVisualizationNodes[detailVisualizationIndex];
        var waveformShape = detailVisualizationNode.querySelector('[data-role="waveform-shape"]');
        var waveformRegion = detailVisualizationNode.querySelector("[data-waveform-state]");
        var waveBars = detailVisualizationNode.querySelector(".waveform-bars");
        var envelopeShape = detailVisualizationNode.querySelector(
          '[data-role="envelope-shape"]'
        );
        var envelopePath = detailVisualizationNode.querySelector(
          '[data-role="envelope-path"]'
        );
        var envelopeValues = detailVisualizationNode.querySelector(
          '[data-role="envelope-values"]'
        );
        detailVisualizations.push({
          id: detailVisualizationNode.getAttribute("data-visualization"),
          kind: detailVisualizationNode.getAttribute(
            "data-visualization-kind"
          ),
          focusable:
            detailVisualizationNode.getAttribute("data-focusable") !== "false",
          focusPath: detailVisualizationNode.getAttribute("data-focus-path"),
          bounds: rectOf(detailVisualizationNode),
          shapeBounds: rectOf(envelopeShape),
          waveform: waveformShape ? {
            state: waveformRegion.getAttribute("data-waveform-state"),
            status: textOf(detailVisualizationNode, '[data-role="waveform-status"]'),
            metadata: textOf(detailVisualizationNode, '[data-role="waveform-metadata"]'),
            asset: textOf(detailVisualizationNode.previousElementSibling, '[data-role="detail-asset-path"]'),
            assetBounds: rectOf(detailVisualizationNode.previousElementSibling),
            bounds: rectOf(waveformShape),
            legendBounds: rectOf(detailVisualizationNode.querySelector(".waveform-legend")),
            trackBounds: rectOf(waveBars),
            labelFont: window.getComputedStyle(detailVisualizationNode.querySelector("figcaption")).fontFamily,
            unavailableBorder: window.getComputedStyle(waveformShape).borderTopStyle,
            bars: Array.from(detailVisualizationNode.querySelectorAll(".waveform-bars > i")).map(function (bar) {
              return {
                top: Number(bar.style.getPropertyValue("--wave-top")),
                height: Number(bar.style.getPropertyValue("--wave-height")),
                color: window.getComputedStyle(bar).backgroundColor,
                bounds: rectOf(bar)
              };
            }),
            landmarks: Array.from(detailVisualizationNode.querySelectorAll("[data-landmark]")).map(function (marker) {
              return {
                role: marker.getAttribute("data-landmark"),
                position: Number(marker.style.getPropertyValue("--landmark-position")),
                label: marker.getAttribute("aria-label"),
                border: window.getComputedStyle(marker).borderLeftStyle,
                bounds: rectOf(marker)
              };
            })
          } : null,
          envelope: envelopeShape
            ? {
                attackMilliseconds: Number(
                  envelopeShape.getAttribute("data-attack-milliseconds")
                ),
                decayMilliseconds: Number(
                  envelopeShape.getAttribute("data-decay-milliseconds")
                ),
                sustain: Number(envelopeShape.getAttribute("data-sustain")),
                releaseMilliseconds: Number(
                  envelopeShape.getAttribute("data-release-milliseconds")
                ),
                timeCoordinate: envelopeShape.getAttribute(
                  "data-time-coordinate"
                ),
                attackMaximumMilliseconds: Number(
                  envelopeShape.getAttribute(
                    "data-attack-maximum-milliseconds"
                  )
                ),
                decayMaximumMilliseconds: Number(
                  envelopeShape.getAttribute(
                    "data-decay-maximum-milliseconds"
                  )
                ),
                releaseMaximumMilliseconds: Number(
                  envelopeShape.getAttribute(
                    "data-release-maximum-milliseconds"
                  )
                ),
                phaseSpanUnits: Number(
                  envelopeShape.getAttribute(
                    "data-phase-span-units"
                  )
                ),
                sustainPreviewSpanUnits: Number(
                  envelopeShape.getAttribute(
                    "data-sustain-preview-span-units"
                  )
                ),
                viewBoxWidthUnits: Number(
                  envelopeShape.getAttribute("data-viewbox-width-units")
                ),
                points: envelopeShape.getAttribute("data-stage-points"),
                path: envelopePath ? envelopePath.getAttribute("d") : null,
                valueText: envelopeValues
                  ? envelopeValues.textContent.replace(/\s+/g, " ").trim()
                  : null,
              }
            : null,
        });
      }
      var slotPositionAttribute = detailNode.getAttribute("data-slot-position");
      var requiredContentOverlapPx = directChildOverlap(detailNode);
      var sectionContainer = detailNode.querySelector(".detail-sections");
      if (sectionContainer) {
        requiredContentOverlapPx = Math.max(
          requiredContentOverlapPx,
          directChildOverlap(sectionContainer)
        );
      }
      for (var overlapSectionIndex = 0; overlapSectionIndex < detailSectionNodes.length; overlapSectionIndex += 1) {
        requiredContentOverlapPx = Math.max(
          requiredContentOverlapPx,
          directChildOverlap(detailSectionNodes[overlapSectionIndex])
        );
      }
      var detailPatchIdAttribute = detailNode.getAttribute("data-patch-id");
      function typeReading(selector) {
        var element = detailNode.querySelector(selector);
        if (!element) return null;
        var style = window.getComputedStyle(element);
        return {
          font: style.fontFamily, size: parseFloat(style.fontSize),
          weight: Number(style.fontWeight), line: parseFloat(style.lineHeight),
          color: style.color
        };
      }
      detail = {
        textScale: Number(computedRoot.getPropertyValue("--shell-text-scale")) || 1,
        typography: {
          assetLabel: typeReading('[data-role="detail-asset-label"]'),
          assetPath: typeReading('[data-role="detail-asset-path"]'),
          subject: typeReading('[data-role="detail-subject"]'),
          sectionLabel: typeReading('[data-role="detail-section-label"]'),
          parameterLabel: typeReading('.prow-label'),
          parameterValue: typeReading('.prow-value'),
          status: typeReading('[data-role="waveform-status"]')
        },
        hierarchy: Array.from(detailNode.children).map(function (child) {
          return child.getAttribute("data-role") || child.getAttribute("data-visualization-kind");
        }),
        surface: textOf(detailNode, '[data-role="detail-title"] .type-label'),
        subjectKind: detailNode.getAttribute("data-detail-kind"),
        patchId:
          detailPatchIdAttribute === null
            ? null
            : Number(detailPatchIdAttribute),
        patch: textOf(detailNode, '[data-role="detail-patch"]'),
        originControl: detailNode.getAttribute("data-origin-control"),
        slotPosition:
          slotPositionAttribute === null ? null : Number(slotPositionAttribute),
        subject: textOf(detailNode, '[data-role="detail-subject"]'),
        status: textOf(detailNode, '[data-role="detail-status"]'),
        controlCount: Number(
          (textOf(detailNode, '[data-role="detail-control-count"]') || "0")
            .split(/\s+/)[0]
        ),
        bounds: rectOf(detailNode),
        horizontalOverflowPx: Math.max(
          0,
          detailNode.scrollWidth - detailNode.clientWidth
        ),
        requiredContentOverlapPx: requiredContentOverlapPx,
        scrollableBy: Math.max(
          0,
          detailNode.scrollHeight - detailNode.clientHeight
        ),
        scrollReachability: scrollReachability(detailNode),
        sections: detailSections,
        rows: rowReport(detailNode.querySelectorAll(".prow[data-focus-path]")),
        visualizations: detailVisualizations,
      };
    }

    var modalNode = doc.getElementById("modal-shell");
    var modal = null;
    if (modalNode) {
      var optionNodes = modalNode.querySelectorAll(".modal-option");
      var modalOptions = [];
      for (var optionIndex = 0; optionIndex < optionNodes.length; optionIndex += 1) {
        var serializedOptionActions = optionNodes[optionIndex].getAttribute(
          "data-valid-actions"
        );
        var optionActions = [];
        if (serializedOptionActions) {
          try {
            optionActions = JSON.parse(serializedOptionActions);
          } catch (_error) {
            optionActions = [{ invalid: true }];
          }
        }
        modalOptions.push({
          identity: optionNodes[optionIndex].getAttribute("data-option-id"),
          focusPath: optionNodes[optionIndex].getAttribute("data-focus-path"),
          focused: optionNodes[optionIndex].classList.contains("is-focused"),
          disabled: optionNodes[optionIndex].classList.contains("is-disabled"),
          enabled: optionNodes[optionIndex].getAttribute("data-enabled") === "true",
          current: optionNodes[optionIndex].getAttribute("data-current") === "true",
          label: textOf(optionNodes[optionIndex], ".modal-option-label"),
          metadata: textOf(optionNodes[optionIndex], ".modal-option-meta"),
          metadataState: optionNodes[optionIndex].getAttribute("data-metadata-state"),
          state:
            optionNodes[optionIndex].getAttribute("data-option-state") ||
            textOf(optionNodes[optionIndex], ".modal-option-state"),
          stateLabel:
            textOf(optionNodes[optionIndex], ".modal-option-availability") ||
            textOf(optionNodes[optionIndex], ".modal-option-state"),
          currentLabel: textOf(optionNodes[optionIndex], ".modal-option-current"),
          validActions: optionActions,
          actionText: textOf(optionNodes[optionIndex], '[data-role="option-actions"]'),
          bounds: rectOf(optionNodes[optionIndex]),
          visible: fullyVisible(optionNodes[optionIndex]),
        });
      }
      var visualizationNodes = modalNode.querySelectorAll("[data-visualization]");
      var modalVisualizations = [];
      for (var modalVisualizationIndex = 0; modalVisualizationIndex < visualizationNodes.length; modalVisualizationIndex += 1) {
        modalVisualizations.push({
          id: visualizationNodes[modalVisualizationIndex].getAttribute("data-visualization"),
          kind: visualizationNodes[modalVisualizationIndex].getAttribute("data-visualization-kind"),
          landmarkCount: visualizationNodes[modalVisualizationIndex].querySelectorAll("[data-landmark]").length,
          pairCount: visualizationNodes[modalVisualizationIndex].querySelectorAll(".waveform-bars > i").length,
          status: textOf(visualizationNodes[modalVisualizationIndex], '[data-role="waveform-status"]'),
        });
      }
      var previewNode = modalNode.querySelector('[data-role="preview-region"]');
      var optionListNode = modalNode.querySelector('[data-role="modal-options"]');
      var optionStatusNode = modalNode.querySelector(
        '[data-role="option-origin-status"]'
      );
      var slotPositionAttribute = modalNode.getAttribute("data-slot-position");
      var modalRequiredOverlapPx = 0;
      if (modalNode.classList.contains("option-modal")) {
        var modalChildren = [];
        for (var modalChildIndex = 0; modalChildIndex < modalNode.children.length; modalChildIndex += 1) {
          var modalChild = modalNode.children[modalChildIndex];
          var modalChildRect = modalChild.getBoundingClientRect();
          if (modalChildRect.width > 0 && modalChildRect.height > 0) {
            modalChildren.push(modalChild);
          }
        }
        for (var modalAdjacentIndex = 0; modalAdjacentIndex + 1 < modalChildren.length; modalAdjacentIndex += 1) {
          var paintedBottom = modalChildren[
            modalAdjacentIndex
          ].getBoundingClientRect().bottom;
          var modalChildStyle = window.getComputedStyle(
            modalChildren[modalAdjacentIndex]
          );
          var clipsDescendants =
            modalChildStyle.overflowY === "auto" ||
            modalChildStyle.overflowY === "scroll" ||
            modalChildStyle.overflowY === "hidden" ||
            modalChildStyle.overflowY === "clip";
          if (!clipsDescendants) {
            var modalDescendants = modalChildren[modalAdjacentIndex].querySelectorAll("*");
            for (var modalDescendantIndex = 0; modalDescendantIndex < modalDescendants.length; modalDescendantIndex += 1) {
              var modalDescendantRect = modalDescendants[
                modalDescendantIndex
              ].getBoundingClientRect();
              if (modalDescendantRect.width > 0 && modalDescendantRect.height > 0) {
                paintedBottom = Math.max(paintedBottom, modalDescendantRect.bottom);
              }
            }
          }
          modalRequiredOverlapPx = Math.max(
            modalRequiredOverlapPx,
            paintedBottom -
              modalChildren[modalAdjacentIndex + 1].getBoundingClientRect().top
          );
        }
      }
      modal = {
        browser: modalNode.classList.contains("sample-browser"),
        title:
          textOf(modalNode, '[data-role="option-title"]') ||
          textOf(modalNode, ".modal-title .type-heading"),
        optionKind: modalNode.getAttribute("data-option-kind"),
        subjectControl: modalNode.getAttribute("data-subject-control"),
        returnControl: modalNode.getAttribute("data-return-control"),
        slotPosition:
          slotPositionAttribute === null ? null : Number(slotPositionAttribute),
        entry: textOf(modalNode, '[data-role="option-entry"]'),
        source: textOf(modalNode, '[data-role="option-source"]'),
        options: modalOptions,
        optionStatus: optionStatusNode
          ? {
              origin: textOf(optionStatusNode, '[data-role="option-origin-label"]'),
              active: textOf(optionStatusNode, '[data-role="option-active"]'),
              requested: textOf(optionStatusNode, '[data-role="option-requested"]'),
              lifecycle: optionStatusNode.getAttribute("data-lifecycle"),
              lifecycleLabel: textOf(
                optionStatusNode,
                '[data-role="option-lifecycle"]'
              ),
              revisions: textOf(
                optionStatusNode,
                '[data-role="option-revisions"]'
              ),
              cause: textOf(optionStatusNode, '[data-role="option-cause"]'),
            }
          : null,
        listBounds: rectOf(optionListNode),
        listReachability: scrollReachability(optionListNode),
        listHorizontalOverflowPx: optionListNode
          ? Math.max(0, optionListNode.scrollWidth - optionListNode.clientWidth)
          : 0,
        requiredContentOverlapPx: Math.max(
          0,
          Math.round(modalRequiredOverlapPx)
        ),
        footerGuidance: textOf(modalNode, '[data-role="option-footer"]'),
        bounds: rectOf(modalNode),
        visualizations: modalVisualizations,
        previewState: previewNode ? previewNode.getAttribute("data-preview-state") : null,
        previewText: previewNode ? textOf(previewNode, '[data-role="preview-playhead"]') : null,
      };
    }

    var lifecycles = [];
    var lifecycleNodes = doc.querySelectorAll("#workspace .lifecycle");
    for (var l = 0; l < lifecycleNodes.length; l += 1) {
      lifecycles.push({
        control: lifecycleNodes[l].getAttribute("data-lifecycle-for"),
        text: lifecycleNodes[l].textContent.replace(/\s+/g, " ").trim(),
      });
    }

    var inspectorElement = doc.getElementById("inspector");
    var sends = [];
    var sendRows = inspectorElement.querySelectorAll(
      '.inspector-controls .prow[data-mixer-kind="send"]'
    );
    for (var r2 = 0; r2 < sendRows.length; r2 += 1) {
      var sendLabel = textOf(sendRows[r2], ".prow-label") || "";
      sends.push({
        trackId: Number(sendRows[r2].getAttribute("data-mixer-track")),
        bus: Number(sendRows[r2].getAttribute("data-mixer-bus")),
        label: sendLabel.replace(/^T[0-9A-Fa-f]{2}\s+/, "").toUpperCase(),
        value: textOf(sendRows[r2], ".prow-value"),
      });
    }
    var utility = rowReport(inspectorElement.querySelectorAll(".prow"));
    var routes = [];
    var routeNodes = inspectorElement.querySelectorAll("[data-route]");
    for (var routeIndex = 0; routeIndex < routeNodes.length; routeIndex += 1) {
      routes.push({
        patchId: routeNodes[routeIndex].getAttribute("data-patch-id"),
        patchName: routeNodes[routeIndex].getAttribute("data-patch-name"),
      });
    }
    var inspectorBody = inspectorElement.querySelector(
      '[data-role="inspector-controls"]'
    );
    var inspectorCorrelation = inspectorElement.querySelector(
      '[data-role="inspector-correlation"]'
    );
    var inspectorFocused = inspectorBody
      ? inspectorBody.querySelector(
          '.prow[data-state="focused"], .prow[data-state="adjusting"]'
        )
      : null;
    var inspectorFocusedVisible = null;
    if (inspectorBody && inspectorFocused) {
      var bodyRect = inspectorBody.getBoundingClientRect();
      var focusedRect = inspectorFocused.getBoundingClientRect();
      inspectorFocusedVisible =
        focusedRect.top >= bodyRect.top - 1 &&
        focusedRect.bottom <= bodyRect.bottom + 1;
    }

    // What the workspace body actually got in the current containing block.
    var bodyNode =
      doc.getElementById("patch-overview") ||
      doc.getElementById("detail") ||
      doc.getElementById("modal-shell");
    var workspaceBody = null;
    if (bodyNode) {
      // The composition's own height, measured from its first child's top to
      // its last child's bottom. `scrollHeight` cannot answer this: it is
      // clamped up to the client height, so a composition that seats reports
      // the band's height rather than its own and the headroom is invisible.
      var kids = bodyNode.children;
      var contentPx = 0;
      if (kids.length > 0) {
        var contentTop = kids[0].getBoundingClientRect().top;
        var contentBottom = kids[kids.length - 1].getBoundingClientRect().bottom;
        var descendants = bodyNode.querySelectorAll("*");
        for (var descendantIndex = 0; descendantIndex < descendants.length; descendantIndex += 1) {
          var descendantRect = descendants[descendantIndex].getBoundingClientRect();
          if (descendantRect.width > 0 && descendantRect.height > 0) {
            contentBottom = Math.max(contentBottom, descendantRect.bottom);
          }
        }
        contentPx = Math.round(contentBottom - contentTop);
      }
      workspaceBody = {
        id: bodyNode.id,
        contentPx: contentPx,
        bandPx: bodyNode.clientHeight,
        scrollableBy: Math.max(
          0,
          bodyNode.scrollHeight - bodyNode.clientHeight
        ),
        widthPx: Math.round(bodyNode.getBoundingClientRect().width),
      };
    }

    var focusedNode = doc.querySelector(
      "#bank .column.focused, #bank .column.correlated"
    );
    var semanticFocusPath = JSON.stringify(model.focusPath || null);
    var semanticFocusedNode = null;
    var semanticFocusMatchCount = 0;
    var semanticNodes = doc.querySelectorAll("[data-focus-path]");
    var targetSizes = [];
    for (var semanticIndex = 0; semanticIndex < semanticNodes.length; semanticIndex += 1) {
      var semanticNode = semanticNodes[semanticIndex];
      if (semanticNode.getAttribute("data-focus-path") === semanticFocusPath) {
        semanticFocusedNode = semanticNode;
        semanticFocusMatchCount += 1;
      }
      targetSizes.push({
        focusPath: semanticNode.getAttribute("data-focus-path"),
        bounds: rectOf(semanticNode),
      });
    }
    var semanticFocusTreatment = null;
    if (semanticFocusedNode) {
      semanticFocusTreatment = semanticFocusedNode.getAttribute(
        "data-focus-treatment"
      );
      if (
        !semanticFocusTreatment &&
        semanticFocusedNode.classList.contains("is-focused")
      ) {
        semanticFocusTreatment = "focused";
      }
    }
    var workspaceElement = doc.getElementById("workspace");
    var workspaceRect = workspaceElement.getBoundingClientRect();
    var inspectorRect = inspectorElement.getBoundingClientRect();
    var overlapWidth = Math.max(
      0,
      Math.min(workspaceRect.right, inspectorRect.right) -
        Math.max(workspaceRect.left, inspectorRect.left)
    );
    var overlapHeight = Math.max(
      0,
      Math.min(workspaceRect.bottom, inspectorRect.bottom) -
        Math.max(workspaceRect.top, inspectorRect.top)
    );
    var actionHintNodes = doc.querySelectorAll('[data-role="action-hint"]');
    var offFooterActionHintCount = 0;
    for (var actionHintIndex = 0; actionHintIndex < actionHintNodes.length; actionHintIndex += 1) {
      if (!actionHintNodes[actionHintIndex].closest("#footer")) {
        offFooterActionHintCount += 1;
      }
    }
    var activeSemanticSurface = surfaceById(model, model.activeSurface);
    var activeSummary = activeSemanticSurface ? activeSemanticSurface.summary : null;
    var fileBrowserSummary = surfaceById(model, "fileBrowser");
    fileBrowserSummary = fileBrowserSummary ? fileBrowserSummary.summary : null;
    var minimumTargetFloor = Number(
      computedRoot.getPropertyValue("--min-interactive-target").replace("px", "").trim()
    );
    var undersizedTargets = targetSizes.filter(function (target) {
      return (
        !target.bounds ||
        target.bounds.widthPx + 0.5 < minimumTargetFloor ||
        target.bounds.heightPx + 0.5 < minimumTargetFloor
      );
    });
    var compatibility = {
      parameterGeneration: model.generation,
      activeGraphRevision:
        model.status && model.status.graphRevision !== undefined
          ? model.status.graphRevision
          : null,
      previewRequestId: fileBrowserSummary
        ? fileBrowserSummary.previewRequestId || null
        : null,
      previewPatchId: fileBrowserSummary ? fileBrowserSummary.patchId : null,
    };
    return {
      generation: model.generation,
      stateHash: model.stateHash,
      deviceScale: window.devicePixelRatio,
      semanticEvidence: {
        context: model.context,
        activeSurface: model.activeSurface,
        focusPath: model.focusPath,
        subject: activeSummary,
        returnIdentity: model.returnPath || null,
        validActions: model.validActions || [],
        observationCompatibility: compatibility,
      },
      bands: bands,
      viewport: {
        widthPx: window.innerWidth,
        heightPx: window.innerHeight,
      },
      layout: {
        mode: layoutMode,
        regions: regions,
        visualOrder: visualRegionOrder,
        workspaceInspectorOverlapPx: Math.round(overlapWidth * overlapHeight),
        horizontalOverflowPx: Math.max(
          0,
          doc.documentElement.scrollWidth - doc.documentElement.clientWidth
        ),
        workspaceHorizontalOverflowPx: Math.max(
          0,
          workspaceElement.scrollWidth - workspaceElement.clientWidth
        ),
        inspectorHorizontalOverflowPx: Math.max(
          0,
          inspectorElement.scrollWidth - inspectorElement.clientWidth
        ),
      },
      workspaceBody: workspaceBody,
      columns: columns,
      rows: rows,
      groups: [],
      stripHeader: null,
      overview: overview,
      detail: detail,
      modal: modal,
      lifecycles: lifecycles,
      focusRepair: textOf(doc, '[data-role="focus-repair"]'),
      sectionAnnotation: textOf(doc, '[data-role="section-annotation"]'),
      patchIdentity: textOf(doc, '[data-role="patch-identity"]'),
      footer: {
        breadcrumb: textOf(doc, '#footer [data-role="breadcrumb"]'),
        guidance: textOf(doc, '#footer [data-role="footer-guidance"]'),
      },
      sampleIdentity: {
        patch: textOf(doc, '[data-role="sample-patch-number"]'),
        name: textOf(doc, '[data-role="sample-patch-name"]'),
        subject: textOf(doc, '[data-role="sample-subject-annotation"]'),
      },
      actionGuidance: {
        count: actionHintNodes.length,
        offFooterCount: offFooterActionHintCount,
      },
      focus: {
        target: textOf(inspectorElement, '[data-role="cursor"]'),
        trackId: focusedNode
          ? Number(focusedNode.getAttribute("data-track"))
          : null,
        semanticPath: semanticFocusedNode
          ? semanticFocusedNode.getAttribute("data-focus-path")
          : null,
        matchCount: semanticFocusMatchCount,
        treatment: semanticFocusTreatment,
        semanticVisible: fullyVisible(semanticFocusedNode),
        targetBounds: rectOf(semanticFocusedNode),
      },
      targetSizes: targetSizes,
      minimumTarget: {
        floorPx: minimumTargetFloor,
        compliant: undersizedTargets.length === 0,
        failureCount: undersizedTargets.length,
      },
      reachability: {
        workspaceScrollableBy: Math.max(
          0,
          workspaceElement.scrollHeight - workspaceElement.clientHeight
        ),
        workspace: modal
          ? modal.listReachability
          : scrollReachability(bodyNode || workspaceElement),
        inspectorScrollableBy: Math.max(
          0,
          inspectorElement.scrollHeight - inspectorElement.clientHeight
        ),
        inspector: scrollReachability(inspectorBody || inspectorElement),
        focusedVisible: fullyVisible(semanticFocusedNode),
      },
      inspector: {
        widthPx: Math.round(inspectorElement.getBoundingClientRect().width),
        cursor: textOf(inspectorElement, '[data-role="cursor"]'),
        bigReadout: textOf(inspectorElement, '[data-role="big-readout"]'),
        mute: textOf(inspectorElement, '[data-role="mute"]'),
        solo: textOf(inspectorElement, '[data-role="solo"]'),
        routes: routes,
        emptyRoute: textOf(inspectorElement, ".empty-route"),
        sends: sends,
        utility: utility,
        controlOrder: utility.map(function (row) {
          return row.control;
        }),
        focusedControl: inspectorFocused
          ? inspectorFocused.getAttribute("data-control")
          : null,
        focusedVisible: inspectorFocusedVisible,
        meter: textOf(inspectorElement, "#inspector-meter-readout"),
        hintLine: textOf(inspectorElement, '[data-role="utility-hint"]'),
        // The outer side region stays pinned while its bounded control body
        // owns responsive scrolling. Measure both facts independently.
        scrollableBy: Math.max(
          0,
          inspectorElement.scrollHeight - inspectorElement.clientHeight
        ),
        scrollReachability: scrollReachability(inspectorBody || inspectorElement),
        bodyScrollableBy: inspectorBody
          ? Math.max(0, inspectorBody.scrollHeight - inspectorBody.clientHeight)
          : 0,
        correlationHeightPx: inspectorCorrelation
          ? Math.round(inspectorCorrelation.getBoundingClientRect().height)
          : 0,
      },
      meter: textOf(doc, "#meter-readout"),
      anatomy: COLUMN_ANATOMY.slice(),
      paintAcknowledgment: paintedEvidence(model),
    };
  }

  // ---- listener glue (kept apart from the pure render) -------------------

  // Presentation-only meter animation state: the document on screen and the
  // latest snapshot frame. Never read by render().
  var latestModel = null;
  var latestFrame = null;
  var meterFrameAt = -Infinity;
  var meterText = { key: null, at: -Infinity };
  var masterClip = { graph: null, count: 0, until: 0 };
  var latestMidiActivity = null;

  function midiActivityValue(message) {
    if (!message) {
      return null;
    }
    if (message.kind === "pitchBend") {
      return Math.min(16383, Number(message.data1) + Number(message.data2) * 128) / 16383;
    }
    if (
      message.kind === "noteOn" ||
      message.kind === "noteOff" ||
      message.kind === "controlChange"
    ) {
      return Number(message.data2) / 127;
    }
    if (message.kind === "programChange" || message.kind === "channelPressure") {
      return Number(message.data1) / 127;
    }
    return null;
  }

  // Repaints only the presentation-owned activity card. Exact revision
  // equality is mandatory; missing/mismatched snapshots remain visibly stale
  // and cannot borrow a prior input's count or message.
  function updateMidiActivity() {
    var doc = window.document;
    var surface = latestModel && midiSettingsSurface(latestModel);
    var inspector = surface && surface.summary && surface.summary.activeInspector;
    var state = doc.getElementById("midi-activity-state");
    var card = doc.getElementById("midi-activity-card");
    if (!state || !card || !inspector) {
      return;
    }
    var observation = latestMidiActivity;
    var snapshot = observation && observation.snapshot;
    var compatible =
      snapshot &&
      JSON.stringify(snapshot.revision) === JSON.stringify(inspector.revision);
    if (!compatible) {
      state.textContent = "WAITING";
      state.setAttribute("data-midi-activity-state", "stale");
      card.setAttribute("data-midi-activity-state", "stale");
      card.innerHTML =
        '<span class="type-label muted">LIVE ACTIVITY</span>' +
        '<span class="type-hint secondary">WAITING FOR MATCHING INPUT</span>';
      return;
    }
    var receiving = Boolean(observation.receiving);
    var event = snapshot.lastEvent;
    var message = event && event.message;
    var position = midiActivityValue(message);
    state.textContent = receiving ? "RECEIVING" : "WAITING";
    state.setAttribute(
      "data-midi-activity-state",
      receiving ? "receiving" : "waiting"
    );
    card.setAttribute(
      "data-midi-activity-state",
      receiving ? "receiving" : "waiting"
    );
    card.innerHTML =
      '<span class="type-label muted">LIVE ACTIVITY</span>' +
      '<span class="type-value secondary">' +
      escapeHtml(String(snapshot.acceptedCount)) +
      " ACCEPTED</span>" +
      (message
        ? '<span class="type-hint secondary">' +
          escapeHtml(String(message.kind).toUpperCase()) +
          " · CH " +
          escapeHtml(String(Number(message.channel) + 1)) +
          " · " +
          escapeHtml(String(message.data1)) +
          " / " +
          escapeHtml(String(message.data2)) +
          "</span>"
        : '<span class="type-hint secondary">NO ACCEPTED MESSAGE</span>') +
      (position === null
        ? ""
        : '<div class="midi-activity-value" data-position="' +
          Math.min(1, Math.max(0, position)).toFixed(6) +
          '"><div class="midi-activity-value-fill"></div></div>');
    applyDynamicGeometry(doc);
  }

  function observeMidiActivity(observation) {
    latestMidiActivity = observation;
    updateMidiActivity();
  }

  function graphRevisionsMatch(frame, model) {
    return (
      JSON.stringify(frame.activeGraphRevision) ===
      JSON.stringify(model.status && model.status.graphRevision)
    );
  }

  function meterReading(frame, model, trackId, now) {
    // Meters describe sounding audio, not navigation or queued MIDI generations.
    // Only a different graph (different routing identity) invalidates the frame.
    var compatible = frame && graphRevisionsMatch(frame, model) && now - meterFrameAt < 500;
    var raw = compatible && (trackId === null ? frame : frame.tracks && frame.tracks[trackId]);
    var level = compatible && frame.metering && (trackId === null ? frame.metering.output : frame.metering.tracks[trackId]);
    var rms = level ? Number(level.rms) : raw ? Number(trackId === null ? raw.outputRms : raw.rms) : NaN;
    var peak = level ? Number(level.peak) : raw ? Math.max(Number(raw.leftPeak), Number(raw.rightPeak)) : NaN;
    if (!Number.isFinite(rms) || !Number.isFinite(peak) || rms < 0 || peak < 0) {
      return { rms: 0, peak: 0, state: "stale" };
    }
    return { rms: rms, peak: peak, state: peak > 1 ? "over" : peak >= 0.001 ? "active" : "zero" };
  }

  function meterFraction(amplitude) {
    return amplitude > 0 ? Math.max(0, Math.min(1, (20 * Math.log10(amplitude) + 60) / 60)) : 0;
  }

  function meterDb(amplitude) {
    return amplitude >= 0.001 ? String(Math.round(20 * Math.log10(amplitude))) : "−∞";
  }

  function paintMeter(node, reading, label) {
    node.style.setProperty("--meter-rms", reading.rms.toFixed(6));
    node.style.setProperty("--meter-level", meterFraction(reading.rms).toFixed(6));
    node.style.setProperty("--meter-peak", meterFraction(reading.peak).toFixed(6));
    node.setAttribute("data-meter-state", reading.state);
    node.setAttribute("aria-label", label + " meter RMS " + meterDb(reading.rms) +
      " dBFS, peak " + meterDb(reading.peak) + " dBFS, " + reading.state);
  }

  function updateMeter() {
    var doc = window.document;
    if (!latestModel) return;
    var now = performance.now();
    var meterNodes = doc.querySelectorAll("[data-meter-track]");
    for (var i = 0; i < meterNodes.length; i += 1) {
      var track = Number(meterNodes[i].getAttribute("data-meter-track"));
      paintMeter(meterNodes[i], meterReading(latestFrame, latestModel, track, now), trackName(track));
    }
    var trackId = focusedTrackId(latestModel);
    var master = doc.querySelector('[data-role="master-meter"]');
    if (trackId === null || !master) return;
    var output = meterReading(latestFrame, latestModel, null, now);
    var selected = meterReading(latestFrame, latestModel, trackId, now);
    paintMeter(master, output, "Master output");
    var graph = latestModel.status && latestModel.status.graphRevision;
    var count = latestFrame && latestFrame.metering ? latestFrame.metering.clippedSamples : 0;
    if (masterClip.graph !== graph || count < masterClip.count) {
      masterClip = { graph: graph, count: count, until: 0 };
    }
    if (output.state !== "stale" && (count > masterClip.count || output.peak > 1)) {
      masterClip.until = now + 2000;
    }
    masterClip.count = count;
    var clipped = output.state !== "stale" && now < masterClip.until;
    master.setAttribute("data-clipped", String(clipped));
    doc.getElementById("master-clip").textContent = output.state === "stale" ? "WAITING" : clipped ? "CLIP" : "0 dBFS";
    var key = String(graph) + ":" + trackId + ":" + selected.state + ":" + output.state;
    // Numeric labels update at 4 Hz; bars retain the 30 Hz observation cadence.
    if (meterText.key !== key || now - meterText.at >= 250) {
      meterText = { key: key, at: now,
        master: output.state === "stale" ? "MASTER — dBFS" : "MASTER " + meterDb(output.peak) + " dBFS",
        selected: selected.state === "stale" ? "RMS — / PEAK — dBFS" :
          "RMS " + meterDb(selected.rms) + " / PEAK " + meterDb(selected.peak) + " dBFS" + (selected.state === "over" ? " · OVER" : "") };
    }
    var caption = doc.getElementById("meter-readout");
    caption.textContent = meterText.master;
    caption.setAttribute("data-meter-state", output.state);
    var inspector = doc.getElementById("inspector-meter-readout");
    if (inspector) {
      inspector.textContent = meterText.selected;
      inspector.setAttribute("data-meter-state", selected.state);
    }
  }

  // Reads preview playback from exactly the same compatible latest-value
  // frame as meters. The model contributes only the canonical correlation
  // identities; the frame contributes only passive playback observation.
  // Neither object is changed during repaint.
  function updatePreviewObservation() {
    var doc = window.document;
    var region = doc.querySelector('[data-role="preview-region"]');
    if (!region || !latestModel) {
      return;
    }
    var browser = surfaceById(latestModel, "fileBrowser");
    var summary = (browser && browser.summary) || {};
    var canonical = summary.preview || { kind: "idle" };
    var expectedIdentity = Number(summary.previewRequestId || 0);
    var expectedPatch = Number(summary.patchId);
    var compatible =
      latestFrame &&
      latestFrame.parameterGeneration === latestModel.generation &&
      graphRevisionsMatch(latestFrame, latestModel) &&
      expectedIdentity > 0 &&
      Number(latestFrame.previewIdentity) === expectedIdentity &&
      Number(latestFrame.previewPatchId) === expectedPatch;
    var playhead = compatible ? Number(latestFrame.previewPlayhead) : 0;
    var finite = Number.isFinite(playhead) && playhead >= 0 && playhead <= 1;
    var state;
    if (canonical.kind !== "playing") {
      state = String(canonical.kind || "idle");
      playhead = 0;
    } else if (!compatible || !finite) {
      state = "stale";
      playhead = 0;
    } else if (!latestFrame.previewPlaying) {
      state = "stopped";
      playhead = 0;
    } else {
      state = "playing";
    }
    region.style.setProperty("--preview-playhead", playhead.toFixed(6));
    region.setAttribute("data-preview-state", state);
    region.setAttribute(
      "aria-label",
      "preview " + state + " playhead " + playhead.toFixed(3)
    );
    var readout = region.querySelector('[data-role="preview-playhead"]');
    if (readout) {
      readout.textContent =
        state === "playing"
          ? "PLAYHEAD " + (playhead * 100).toFixed(1) + "% / PLAYING"
          : "PLAYHEAD — / " + state.toUpperCase();
    }
  }

  function observeAudio(frame) {
    if (!latestFrame || frame.sequence !== latestFrame.sequence || frame.activeGraphRevision !== latestFrame.activeGraphRevision) meterFrameAt = performance.now();
    latestFrame = frame;
    updateMeter();
    updatePreviewObservation();
  }

  // The paint acknowledgment for one painted document: the document's
  // semantic identity — generation, stateHash, context, active surface,
  // focus path, interaction mode — copied verbatim from the received
  // document (never invented, cached, or re-derived), plus the post-paint
  // measured viewport and per-region bounds the adapter's
  // ShellFrameObservation forwarding consumes (WP02). The region ids spell
  // ShellRegionId's serialized names so the Rust side can assemble honest
  // observations without re-deriving them.
  //
  // The legacy optional `strip` transport field remains absent for the
  // Overview. Detailed section, control, target, and reflow evidence is read
  // through `renderObservation`; the five canonical region measurements
  // continue through this production acknowledgment.
  function paintedEvidence(model) {
    var doc = window.document;
    var bands = [
      { id: "contextLine", element: "context-line" },
      { id: "identityHeader", element: "identity-header" },
      { id: "mainWorkspace", element: "workspace" },
      { id: "persistentSideRegion", element: "inspector" },
      { id: "footer", element: "footer" },
    ];
    var regions = [];
    for (var i = 0; i < bands.length; i += 1) {
      var el = doc.getElementById(bands[i].element);
      if (!el) {
        continue;
      }
      var rect = el.getBoundingClientRect();
      var label = "";
      var nodes = el.querySelectorAll("span");
      for (var n = 0; n < nodes.length; n += 1) {
        var text = nodes[n].textContent.replace(/\s+/g, " ").trim();
        if (text) {
          label = text;
          break;
        }
      }
      regions.push({
        id: bands[i].id,
        xPx: rect.x,
        yPx: rect.y,
        widthPx: rect.width,
        heightPx: rect.height,
        label: label,
      });
    }
    return {
      generation: model.generation,
      stateHash: model.stateHash,
      context: model.context,
      activeSurface: model.activeSurface,
      focusPath: model.focusPath,
      interactionMode: model.interactionMode,
      viewport: {
        widthPx: window.innerWidth,
        heightPx: window.innerHeight,
      },
      deviceScale: window.devicePixelRatio,
      regions: regions,
    };
  }

  // The page half of the typed render-failure path: one small
  // JSON-serializable payload — error name, message, and the failing
  // document's semantic identity (the same generation/stateHash pair the
  // painted ack leads with) when a document is available — emitted on
  // crest://render-error, which the shell converts to the fatal typed
  // PageRenderFailed error. First error wins: the shell treats the first
  // payload as fatal, so the latch drops every later fault instead of
  // letting it replace the recorded failure. The boundary itself does no
  // rendering, no DOM access, and no formatting beyond these strings, so it
  // cannot throw before emitting.
  var renderErrorEmitted = false;

  function emitRenderError(error, model) {
    if (renderErrorEmitted) {
      return; // first error wins; the shell treats the first payload as fatal
    }
    renderErrorEmitted = true;
    var tauri = window.__TAURI__;
    if (!tauri || !tauri.event) {
      return; // headless harness: the throw already propagated to the caller
    }
    tauri.event.emit(RENDER_ERROR_EVENT, {
      name: error && error.name ? String(error.name) : "Error",
      message: error && error.message ? String(error.message) : String(error),
      generation:
        model && model.generation !== undefined ? model.generation : null,
      stateHash:
        model && model.stateHash !== undefined ? model.stateHash : null,
    });
  }

  function reportRenderTime(start) {
    if (start !== null) {
      window.__TAURI__.event.emit("crest://performance", performance.now() - start).catch(function () {});
    }
  }

  function attachTransports() {
    var tauri = window.__TAURI__;
    if (!tauri || !tauri.event) {
      return; // headless harness drives window.crest.render directly
    }
    var projectionListener = tauri.event.listen(
      PROJECTION_EVENT,
      function (event) {
        var model = event.payload;
        latestModel = model;
        // A document that fails to render must NOT ack: the catch emits the
        // typed crest://render-error payload (fatal shell-side) and returns
        // before the acknowledgment below is ever scheduled.
        var renderStart = window.__CREST_PERF__ ? performance.now() : null;
        try {
          render(model);
          updateMeter();
          updatePreviewObservation();
          updateMidiActivity();
        } catch (error) {
          emitRenderError(error, model);
          return; // a failed render must NOT ack
        }
        reportRenderTime(renderStart);
        // Exactly one paint ack per painted document, in paint order: emitted
        // after this frame has painted, carrying the document's own identity
        // with post-paint region evidence.
        window.requestAnimationFrame(function () {
          // A hosted page can be hidden after render but before this callback.
          // That frame was not painted; zero-sized hidden bounds are not evidence.
          // Re-entry republishes the current Synth projection through the host.
          var hostedRoot = window.document.getElementById("synth-root");
          if (activeWorkspace || (hostedRoot && hostedRoot.hidden)) {
            return;
          }
          tauri.event
            .emit(PAINTED_EVENT, paintedEvidence(model))
            .catch(function (error) {
              emitRenderError(error, model);
            });
        });
      }
    );
    var meterListener = tauri.event.listen(METER_EVENT, function (event) {
      observeAudio(event.payload);
    });
    var midiActivityListener = tauri.event.listen(
      MIDI_ACTIVITY_EVENT,
      function (event) {
        observeMidiActivity(event.payload);
      }
    );
    var sessionDocumentListener = tauri.event.listen(SESSION_DOCUMENT_EVENT, function (event) {
      observeSessionDocument(event.payload);
    });
    var workspaceListener = tauri.event.listen("crest://host-projection", function (event) {
      var renderStart = window.__CREST_PERF__ ? performance.now() : null;
      try { observeWorkspace(event.payload); } catch (error) { emitRenderError(error, null); }
      reportRenderTime(renderStart);
    });
    Promise.all([projectionListener, meterListener, midiActivityListener, sessionDocumentListener, workspaceListener])
      .then(function () {
        return tauri.event.emit(READY_EVENT, { ready: true });
      })
      .catch(function (error) {
        emitRenderError(error, latestModel);
      });
  }

  // Any uncaught page fault after load — including a throw inside the
  // requestAnimationFrame ack callback (paintedEvidence) — is the same typed
  // fatal condition. Both handlers share the emitRenderError first-error
  // latch with the projection listener's boundary. These are fault
  // reporters, not input capture: the page still registers no key handler.
  window.addEventListener("error", function (event) {
    emitRenderError(
      event.error || { name: "Error", message: event.message },
      latestModel
    );
  });
  window.addEventListener("unhandledrejection", function (event) {
    emitRenderError(
      event.reason instanceof Error
        ? event.reason
        : { name: "UnhandledRejection", message: String(event.reason) },
      latestModel
    );
  });

  window.addEventListener("resize", function () {
    if (activeWorkspace) {
      reportWorkspacePageSize(activeWorkspace);
      return;
    }
    if (latestModel) {
      revealSemanticFocus(window.document, latestModel);
    }
  });

  window.crest = {
    render: render,
    observeWorkspace: observeWorkspace,
    renderObservation: renderObservation,
    observeAudio: observeAudio,
    observeMidiActivity: observeMidiActivity,
    observeSessionDocument: observeSessionDocument,
  };
  attachTransports();
})();
