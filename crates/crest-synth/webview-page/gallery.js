// crest-synth component gallery render script (WP04, spec C-006: rebuild).
//
// PURE RENDER. `render(doc)` rebuilds the gallery from one gallery-scene
// document (schema crest-gallery-scene/1) and nothing else: no Date.now, no
// Math.random, no accumulated state, no iteration-order dependence beyond
// the document's own array order. The same document always paints the same
// DOM. The document is scene-local — it is NOT a SemanticGraphicalViewModel
// and never claims that schema; the Rust scene declares it beside the
// production model (spec C-002/C-006).
//
// This page registers no key handler and captures no input of any kind —
// digit and bracket paging stays Rust-side and scene-local. Gallery page
// selection never becomes a SemanticAction.
//
// After each paint the page reads its own painted DOM back and emits one
// acknowledgment on crest://gallery-painted carrying the page, state, and
// viewport identity actually rendered — copied from the DOM, never from the
// input document — so the scene's observation is ack-gated evidence rather
// than a pre-render plan.
"use strict";

(function galleryModule() {
  var GALLERY_EVENT = "crest://gallery";
  var PAINTED_EVENT = "crest://gallery-painted";
  var READY_EVENT = "crest://gallery-ready";
  var SCHEMA = "crest-gallery-scene/1";

  // The authored hint separator (primitives::hint::HINT_SEPARATOR), reused
  // for the composition state lines.
  var HINT_SEPARATOR = " · ";

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

  function toneClass(tone) {
    return tone ? " " + escapeHtml(tone) : "";
  }

  // ---- specimen renderers (pure: document in, HTML string out) ----------

  function colorHtml(s) {
    return (
      '<div class="g-row" data-specimen="color" data-color="' +
      escapeHtml(s.name) +
      '" style="--g-swatch: var(' +
      escapeHtml(s.cssVar) +
      ')">' +
      '<span class="g-swatch"></span>' +
      '<span class="g-name type-label">' +
      escapeHtml(s.name) +
      "</span>" +
      '<span class="spring"></span>' +
      '<span class="g-caption type-value secondary">' +
      escapeHtml(s.hex) +
      "</span>" +
      "</div>"
    );
  }

  function typeHtml(s) {
    return (
      '<div class="g-row" data-specimen="type" data-type="' +
      escapeHtml(s.name) +
      '">' +
      '<span class="' +
      escapeHtml(s.cssClass) +
      toneClass(s.toneClass) +
      '">' +
      escapeHtml(s.sample) +
      "</span>" +
      '<span class="spring"></span>' +
      '<span class="g-caption type-hint muted">' +
      escapeHtml(s.name) +
      HINT_SEPARATOR +
      escapeHtml(String(s.sizePx)) +
      "/" +
      escapeHtml(String(s.lineHeightPx)) +
      HINT_SEPARATOR +
      escapeHtml(String(s.weight)) +
      "</span>" +
      "</div>"
    );
  }

  function metricHtml(s) {
    return (
      '<div class="g-row" data-specimen="metric" data-metric="' +
      escapeHtml(s.name) +
      '">' +
      '<span class="g-metric" data-axis="' +
      escapeHtml(s.axis) +
      '" style="--g-metric: var(' +
      escapeHtml(s.cssVar) +
      ')"></span>' +
      '<span class="g-name type-label">' +
      escapeHtml(s.name) +
      "</span>" +
      '<span class="spring"></span>' +
      '<span class="g-caption type-value secondary">' +
      escapeHtml(String(s.px)) +
      " px</span>" +
      "</div>"
    );
  }

  function valueHtml(s) {
    return (
      '<div class="g-row" data-specimen="value">' +
      '<span class="g-name type-label">' +
      escapeHtml(s.label) +
      "</span>" +
      '<span class="spring"></span>' +
      '<span class="type-value' +
      toneClass(s.toneClass) +
      '">' +
      escapeHtml(s.value) +
      "</span>" +
      "</div>"
    );
  }

  function hintHtml(s) {
    return (
      '<div class="g-row" data-specimen="hint" data-tone="' +
      escapeHtml(s.tone) +
      '">' +
      '<span class="type-hint" style="color: var(' +
      escapeHtml(s.accentVar) +
      ')">' +
      escapeHtml(s.chord) +
      " " +
      escapeHtml(s.action) +
      "</span>" +
      '<span class="spring"></span>' +
      '<span class="g-caption type-hint muted">' +
      escapeHtml(s.tone) +
      "</span>" +
      "</div>"
    );
  }

  // The colorless evidence a painted state row carries, written onto the row
  // so the ack can copy it back from the DOM.
  function stateEvidence(s) {
    var evidence = "keyline " + s.keylinePx + " px";
    if (s.halo) {
      evidence += HINT_SEPARATOR + "halo";
    }
    if (s.fillsRow) {
      evidence += HINT_SEPARATOR + "row fill";
    }
    if (s.cursor) {
      evidence += HINT_SEPARATOR + "cursor >";
    }
    if (s.mark !== null && s.mark !== undefined) {
      evidence += HINT_SEPARATOR + "mark " + s.mark;
    }
    return evidence;
  }

  // The presentation body between a state row's label and its value.
  function statePresentationHtml(s) {
    var level =
      typeof s.level === "number" ? Number(s.level).toFixed(6) : null;
    if (s.presentation === "toggle") {
      return (
        '<span class="g-toggle-pill type-label">' +
        escapeHtml(s.value) +
        "</span>"
      );
    }
    if (s.presentation === "slider" || s.presentation === "meter") {
      return level === null
        ? ""
        : '<span class="g-bar" style="--g-level:' +
            level +
            '"><span class="g-bar-fill"></span></span>';
    }
    if (s.presentation === "fader") {
      return level === null
        ? ""
        : '<span class="g-fader" style="--level:' +
            level +
            '"><span class="fader-track"><span class="fader-fill"></span>' +
            '<span class="fader-cap"></span></span></span>';
    }
    if (s.presentation === "row" && level !== null) {
      return (
        '<span class="g-bar" style="--g-level:' +
        level +
        '"><span class="g-bar-fill"></span></span>'
      );
    }
    return "";
  }

  function stateHtml(s) {
    var selection =
      s.fillsRow && s.mark !== null && s.mark !== undefined
        ? '<span class="g-selection-square"></span>'
        : "";
    var mark =
      s.mark !== null && s.mark !== undefined && !s.fillsRow
        ? '<span class="g-mark type-hint">' + escapeHtml(s.mark) + "</span>"
        : "";
    return (
      '<div class="g-state" data-specimen="state" data-state-name="' +
      escapeHtml(s.state) +
      '" data-control="' +
      escapeHtml(s.control === null || s.control === undefined ? "" : s.control) +
      '" data-halo="' +
      (s.halo ? "true" : "false") +
      '" data-fill="' +
      (s.fillsRow ? "true" : "false") +
      '" data-evidence="' +
      escapeHtml(stateEvidence(s)) +
      '" style="--g-accent: var(' +
      escapeHtml(s.accentVar) +
      "); --g-keyline: var(" +
      escapeHtml(s.keylineVar) +
      ')">' +
      '<span class="g-cursor type-label">' +
      (s.cursor ? "&gt;" : "") +
      "</span>" +
      '<span class="g-label type-label">' +
      escapeHtml(s.label) +
      "</span>" +
      statePresentationHtml(s) +
      '<span class="spring"></span>' +
      selection +
      mark +
      '<span class="g-value type-value">' +
      escapeHtml(s.value) +
      "</span>" +
      "</div>"
    );
  }

  function bandHtml(s) {
    return (
      '<div class="g-band" data-band-region="' +
      escapeHtml(s.region) +
      '" style="--g-weight:' +
      Number(s.weight) +
      '">' +
      '<span class="type-hint secondary">' +
      escapeHtml(s.label) +
      "</span>" +
      "</div>"
    );
  }

  // One mixer track column: the declared five-structure anatomy in order,
  // through the same page.css classes and data-structure attributes the
  // shipped MIXER page paints (MixerTrackColumnStructure).
  function bankColumnHtml(column) {
    var level = Number(column.level).toFixed(6);
    return (
      '<div class="column' +
      (column.focused ? " focused" : "") +
      '" data-track="' +
      escapeHtml(column.track) +
      '">' +
      '<span class="structure track-header type-label ' +
      (column.focused ? "focus" : "secondary") +
      '" data-structure="TrackHeader">' +
      escapeHtml(column.track) +
      "</span>" +
      '<div class="structure level-fader" data-structure="LevelFader" data-state="' +
      (column.focused ? "focused" : "resting") +
      '" style="--level:' +
      level +
      '"><div class="fader-track"><div class="fader-fill"></div>' +
      '<div class="fader-cap"></div></div></div>' +
      '<span class="structure level-readout type-value ' +
      (column.focused ? "focus" : "patch") +
      '" data-structure="LevelReadout">' +
      escapeHtml(column.levelHex) +
      "</span>" +
      '<span class="structure pan-readout type-hint" data-structure="PanReadout">' +
      '<span class="muted">P</span> <span class="secondary">' +
      escapeHtml(column.pan) +
      "</span></span>" +
      '<span class="structure state-line type-hint secondary" data-structure="StateLine">' +
      escapeHtml(column.stateLine) +
      "</span>" +
      "</div>"
    );
  }

  function compositionEntryHtml(entry) {
    var value =
      entry.value === null || entry.value === undefined
        ? ""
        : '<span class="spring"></span><span class="type-value secondary">' +
          escapeHtml(entry.value) +
          "</span>";
    return (
      '<div class="g-comp-row">' +
      '<span class="type-label' +
      toneClass(entry.toneClass) +
      '">' +
      escapeHtml(entry.label) +
      "</span>" +
      value +
      "</div>"
    );
  }

  function compositionHtml(s) {
    var body = "";
    if (s.form === "bank") {
      var columns = "";
      for (var i = 0; i < s.columns.length; i += 1) {
        columns += bankColumnHtml(s.columns[i]);
      }
      body = '<div class="g-bank">' + columns + "</div>";
    } else if (s.form === "frame") {
      var bands = "";
      for (var b = 0; b < s.entries.length; b += 1) {
        bands +=
          '<div class="g-comp-row"><span class="type-hint muted">' +
          escapeHtml(s.entries[b].label) +
          "</span></div>";
      }
      body = bands;
    } else {
      for (var e = 0; e < s.entries.length; e += 1) {
        body += compositionEntryHtml(s.entries[e]);
      }
    }
    return (
      '<div class="g-comp" data-specimen="composition" data-composition="' +
      escapeHtml(s.composition) +
      '" data-region="' +
      escapeHtml(s.region) +
      '" data-form="' +
      escapeHtml(s.form) +
      '">' +
      body +
      "</div>"
    );
  }

  // Render dispatch over the document's closed specimen kinds. An unknown
  // kind renders an explicit visible marker, never a silent blank.
  function specimenHtml(s) {
    if (s.kind === "color") {
      return colorHtml(s);
    }
    if (s.kind === "type") {
      return typeHtml(s);
    }
    if (s.kind === "metric") {
      return metricHtml(s);
    }
    if (s.kind === "value") {
      return valueHtml(s);
    }
    if (s.kind === "hint") {
      return hintHtml(s);
    }
    if (s.kind === "state") {
      return stateHtml(s);
    }
    if (s.kind === "composition") {
      return compositionHtml(s);
    }
    return (
      '<div class="g-row warning" data-specimen="unknown">?' +
      escapeHtml(String(s.kind)) +
      "</div>"
    );
  }

  function sectionHtml(section) {
    var specimens = "";
    var bands = "";
    for (var i = 0; i < section.specimens.length; i += 1) {
      var s = section.specimens[i];
      if (s.kind === "band") {
        bands += bandHtml(s);
      } else {
        specimens += specimenHtml(s);
      }
    }
    if (bands) {
      specimens += '<div class="g-bands">' + bands + "</div>";
    }
    return (
      '<section class="g-section">' +
      '<span class="g-section-heading type-hint muted">' +
      escapeHtml(section.heading) +
      "</span>" +
      specimens +
      "</section>"
    );
  }

  function densityHtml(column) {
    var sections = "";
    for (var i = 0; i < column.sections.length; i += 1) {
      sections += sectionHtml(column.sections[i]);
    }
    return (
      '<section class="g-density" data-density="' +
      escapeHtml(column.policy) +
      '" style="--g-colw: var(' +
      escapeHtml(column.columnWidthVar) +
      ')">' +
      '<span class="g-density-caption type-label muted">' +
      escapeHtml(column.label) +
      "</span>" +
      sections +
      "</section>"
    );
  }

  // ---- the pure render ---------------------------------------------------

  // Rebuilds the gallery from one gallery-scene document. Same document,
  // identical DOM. The active page identity — title, page number, digit —
  // is always on screen, with the full fifteen-entry index beneath it.
  function render(doc) {
    var root = window.document.getElementById("gallery");
    root.setAttribute("data-page", doc.page);
    root.setAttribute("data-title", doc.title);
    var digit =
      doc.digitLabel === null || doc.digitLabel === undefined
        ? '<span class="type-label muted">STEP ONLY [ ]</span>'
        : '<span class="type-label focus">DIGIT ' +
          escapeHtml(doc.digitLabel) +
          "</span>";
    var header =
      '<header id="gallery-header">' +
      '<span class="type-heading" data-role="page-title">' +
      escapeHtml(doc.title) +
      "</span>" +
      '<span class="type-label muted" data-role="page-number">PAGE ' +
      escapeHtml(String(doc.pageNumber)) +
      " / " +
      escapeHtml(String(doc.pagesDeclared)) +
      "</span>" +
      digit +
      '<span class="spring"></span>' +
      '<span class="type-hint muted">' +
      escapeHtml(doc.stepHint) +
      "</span>" +
      "</header>";
    var index = "";
    for (var i = 0; i < doc.index.length; i += 1) {
      var entry = doc.index[i];
      index +=
        '<span class="g-index-entry type-hint ' +
        (entry.active ? "focus" : "muted") +
        '" data-index-page="' +
        escapeHtml(entry.page) +
        '"' +
        (entry.active ? ' data-active="true"' : "") +
        ">" +
        escapeHtml(entry.key) +
        " " +
        escapeHtml(entry.label) +
        "</span>";
    }
    var densities = "";
    for (var d = 0; d < doc.densities.length; d += 1) {
      densities += densityHtml(doc.densities[d]);
    }
    root.innerHTML =
      header +
      '<nav id="gallery-index">' +
      index +
      "</nav>" +
      '<div id="gallery-densities">' +
      densities +
      "</div>";
  }

  // ---- the painted acknowledgment (read back from the DOM) ---------------

  function painted(el) {
    if (!el) {
      return false;
    }
    var rect = el.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  }

  function textOf(el) {
    return el ? el.textContent.replace(/\s+/g, " ").trim() : "";
  }

  // Every distinct computed color painted inside `root`: text, backgrounds,
  // and the keyline borders. Transparent values are skipped — they paint
  // nothing.
  function paintedColors(root) {
    var seen = {};
    var out = [];
    var nodes = root.querySelectorAll("*");
    var record = function (color) {
      if (!color || color === "rgba(0, 0, 0, 0)" || color === "transparent") {
        return;
      }
      if (!Object.prototype.hasOwnProperty.call(seen, color)) {
        seen[color] = true;
        out.push(color);
      }
    };
    for (var i = 0; i < nodes.length; i += 1) {
      var style = window.getComputedStyle(nodes[i]);
      record(style.color);
      record(style.backgroundColor);
      record(style.borderLeftColor);
      record(style.borderBottomColor);
    }
    return out;
  }

  // Clipped or invisible specimens, named so a failure is actionable. Flex
  // rows with overflow hidden clip rather than overlap, so clipping is the
  // measurable defect; a specimen with no painted area is the other.
  function paintDefects(root) {
    var defects = [];
    var texts = root.querySelectorAll(
      ".g-label, .g-value, .g-name, .g-caption, .g-mark, .g-section-heading"
    );
    for (var i = 0; i < texts.length; i += 1) {
      var el = texts[i];
      if (el.scrollWidth > el.clientWidth + 1) {
        defects.push("clipped: " + textOf(el));
      }
    }
    var specimens = root.querySelectorAll("[data-specimen]");
    for (var s = 0; s < specimens.length; s += 1) {
      if (!painted(specimens[s])) {
        defects.push(
          "nothing visible: " +
            (specimens[s].getAttribute("data-state-name") ||
              specimens[s].getAttribute("data-composition") ||
              specimens[s].getAttribute("data-specimen"))
        );
      }
    }
    return defects;
  }

  // Whether the vendored authored faces resolved for the painted text. The
  // ready handshake below waits for the font faces before the first render,
  // so a false here is a real resolution failure, not a race.
  function typefaceResolved(root) {
    var fonts = window.document.fonts;
    if (!fonts || !fonts.check) {
      return false;
    }
    var faces =
      fonts.check('400 15px "Azeret Mono"') &&
      fonts.check('500 12px "Azeret Mono"') &&
      fonts.check('600 14px "Azeret Mono"') &&
      fonts.check('700 14px "Azeret Mono"');
    var sample = root.querySelector(".g-label, .g-name, .type-label");
    var family = sample
      ? window.getComputedStyle(sample).fontFamily
      : window.getComputedStyle(window.document.body).fontFamily;
    return faces && family.indexOf("Azeret Mono") !== -1;
  }

  // One density column's painted evidence, read from its DOM.
  function densityAck(column) {
    var states = [];
    var stateNodes = column.querySelectorAll('[data-specimen="state"]');
    var controlsByName = {};
    var controls = [];
    for (var i = 0; i < stateNodes.length; i += 1) {
      var node = stateNodes[i];
      var label = textOf(node.querySelector(".g-label"));
      states.push({
        state: node.getAttribute("data-state-name"),
        label: label,
        evidence: node.getAttribute("data-evidence"),
      });
      var control = node.getAttribute("data-control");
      if (control) {
        if (!Object.prototype.hasOwnProperty.call(controlsByName, control)) {
          controlsByName[control] = {
            control: control,
            states: [],
            label: label,
          };
          controls.push(controlsByName[control]);
        }
        controlsByName[control].states.push(
          node.getAttribute("data-state-name")
        );
      }
    }
    var compositions = [];
    var compositionNodes = column.querySelectorAll(
      '[data-specimen="composition"]'
    );
    var bank = null;
    for (var c = 0; c < compositionNodes.length; c += 1) {
      var comp = compositionNodes[c];
      compositions.push({
        composition: comp.getAttribute("data-composition"),
        region: comp.getAttribute("data-region"),
        label: textOf(comp.querySelector("span")),
      });
      if (comp.getAttribute("data-form") === "bank") {
        var firstColumn = comp.querySelector(".column");
        var structures = [];
        if (firstColumn) {
          var structureNodes = firstColumn.querySelectorAll("[data-structure]");
          for (var t = 0; t < structureNodes.length; t += 1) {
            structures.push(structureNodes[t].getAttribute("data-structure"));
          }
        }
        bank = {
          structures: structures,
          levelReadout: textOf(
            firstColumn &&
              firstColumn.querySelector('[data-structure="LevelReadout"]')
          ),
        };
      }
    }
    var bands = [];
    var bandNodes = column.querySelectorAll("[data-band-region]");
    for (var b = 0; b < bandNodes.length; b += 1) {
      if (painted(bandNodes[b])) {
        bands.push(bandNodes[b].getAttribute("data-band-region"));
      }
    }
    return {
      policy: column.getAttribute("data-density"),
      painted: painted(column),
      states: states,
      controls: controls,
      compositions: compositions,
      bands: bands,
      bank: bank,
    };
  }

  // The complete painted acknowledgment: everything is copied from the
  // painted DOM — page identity from the root's rendered attributes, states
  // and viewports from the painted columns — never from the input document.
  function buildAck() {
    var doc = window.document;
    var root = doc.getElementById("gallery");
    var title = textOf(doc.querySelector('[data-role="page-title"]'));
    var header = doc.getElementById("gallery-header");
    var densities = [];
    var columns = root.querySelectorAll(".g-density");
    for (var i = 0; i < columns.length; i += 1) {
      densities.push(densityAck(columns[i]));
    }
    return {
      page: root.getAttribute("data-page") || "",
      titleVisible:
        painted(header) &&
        title.length > 0 &&
        title === (root.getAttribute("data-title") || ""),
      viewport: {
        widthPx: window.innerWidth,
        heightPx: window.innerHeight,
      },
      typefaceResolved: typefaceResolved(root),
      paintedColors: paintedColors(root),
      defects: paintDefects(root),
      densities: densities,
    };
  }

  // ---- listener glue (kept apart from the pure render) -------------------

  function attachTransports() {
    var tauri = window.__TAURI__;
    if (!tauri || !tauri.event) {
      return; // headless harness drives window.crestGallery directly
    }
    tauri.event.listen(GALLERY_EVENT, function (event) {
      var doc = event.payload;
      if (!doc || doc.schema !== SCHEMA) {
        // A document this page does not understand is refused, never
        // guessed at — and refusing means no ack, so the scene sees the
        // page as unrendered rather than as painted.
        return;
      }
      render(doc);
      // Exactly one paint ack per painted document, in paint order, emitted
      // after this frame has painted, carrying identity read back from the
      // painted DOM.
      window.requestAnimationFrame(function () {
        tauri.event.emit(PAINTED_EVENT, buildAck());
      });
    });
    var announce = function () {
      tauri.event.emit(READY_EVENT, {});
    };
    // The faces are part of readiness: the first document must paint in the
    // authored typeface, not in a fallback that resolves a frame later.
    if (window.document.fonts && window.document.fonts.ready) {
      window.document.fonts.ready.then(announce);
    } else {
      announce();
    }
  }

  window.crestGallery = { render: render, buildAck: buildAck };
  attachTransports();
})();
