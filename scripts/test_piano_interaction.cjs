const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { test } = require("node:test");
const vm = require("node:vm");

const page = file => fs.readFileSync(path.join(__dirname, "../webview-page", file), "utf8");
const phraseView = vm.runInNewContext(
  ["wire-values.js", "time-values.js", "phrase-view.js"].map(page).join("\n") + "; CrestTrackerPhraseView"
);
const host = page("host.js");
function hostFunction(name, next) {
  return host.slice(host.indexOf("  function " + name + "("), host.indexOf("  function " + next + "("));
}

// Minimal DOM for the production render/retention and pointer handlers. Musical
// geometry comes from the real PianoTimeWindow, not a duplicate test model.
class Element {
  constructor() {
    this.children = [];
    this.style = { setProperty() {} };
    this.dataset = {};
    this.className = "";
    this.clientWidth = 1664;
    this.scrollLeft = 0;
    this.scrollTop = 0;
  }
  append(...nodes) {
    for (const node of nodes) {
      node.parentElement = this;
      this.children.push(node);
    }
  }
  replaceChildren(...nodes) { this.children = []; this.append(...nodes); }
  querySelector(selector) {
    for (const node of this.children) {
      if (node.className.split(" ").includes(selector.slice(1))) return node;
      const child = node.querySelector(selector);
      if (child) return child;
    }
    return null;
  }
  setAttribute() {}
  getBoundingClientRect() { return { left: 20 }; }
}

const beat = numerator => ({ numerator, denominator: 1 });
function pianoHarness() {
  const container = new Element(), emitted = [], noop = () => {};
  const phrase = { steps: 512, events: [], form: { kind: "piano_roll" } };
  const context = {
    PIANO_LABEL_WIDTH_PX: phraseView.PIANO_LABEL_WIDTH_PX,
    Map, Set, Math, Number, String, document: { createElement: () => new Element() },
    $: () => container,
    model: { phrase: { id: 1, value: phrase }, revision: 1, selectedEvents: [],
      pianoControls: { at: beat(0), pitch: 60, grid: beat(1) } },
    pianoSectionPhrase: null, pianoAnchor: beat(0), lastPianoCursor: null,
    pianoRenderKey: null, pianoViewport: null, pianoZoom: 1, pianoFollow: false,
    pianoExpressionKind: "bend", pianoExpressionScope: "expression",
    stringify: value => JSON.stringify(value, (_, value) => typeof value === "bigint" ? String(value) : value),
    compareBeats: (a, b) => Number(BigInt(a.numerator) * BigInt(b.denominator) - BigInt(b.numerator) * BigInt(a.denominator)),
    pianoPlaybackBeat: () => null, renderPianoWindow: noop, updatePianoCapture: noop,
    updatePianoSelection: noop, updatePianoViewport: noop, updatePianoTicks: noop,
    followPianoPlayback: noop, noteName: String, renderPianoEventList: () => new Element(),
    emit: value => emitted.push(value), clearEventSelection: noop,
    noteDialog: (...args) => emitted.push({ insert: args[4] }),
    pianoContentSource: () => ({ storedEvents: new Set(), summary: { notes: [], low: 60, high: 72 }, index: {} }),
  };
  context.updatePianoCursor = () => {
    context.lastPianoCursor = context.stringify([1, context.model.pianoControls.at, 60]);
  };
  context.pianoMetrics = () => new phraseView.PianoTimeWindow({
    total: beat(128), anchor: context.pianoAnchor, viewportWidth: container.clientWidth, zoom: context.pianoZoom,
  });
  vm.createContext(context);
  vm.runInContext(hostFunction("renderPiano", "updatePianoTicks") +
    hostFunction("updatePianoTimeWindow", "pianoPointerAction") +
    hostFunction("pianoPointerAction", "updatePianoSelection"), context);
  return { context, container, emitted };
}

test("retained Piano rows use visible section time for cursor and note insertion", () => {
  const { context, container, emitted } = pianoHarness();
  context.renderPiano();
  const row = container.querySelector(".piano-pitch");
  for (const [anchor, expected] of [[64, 32], [96, 64], [0, 0]]) {
    context.pianoAnchor = beat(anchor);
    context.renderPiano({ follow: false });
    assert.equal(container.querySelector(".piano-pitch"), row, "the actual hit target survives paging");
    assert.equal(context.pianoViewport.metrics.start.numerator, BigInt(expected));
    row.onclick({ clientX: 84 });
    row.ondblclick({ clientX: 184 });
    assert.equal(BigInt(emitted.at(-2).control.at.numerator), BigInt(expected));
    assert.equal(BigInt(emitted.at(-1).insert.numerator), BigInt(expected + 1));
  }
  assert.deepEqual(context.model.phrase.value.events, [], "pointer positioning alone never mutates source events");
});
