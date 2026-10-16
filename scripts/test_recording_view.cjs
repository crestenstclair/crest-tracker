const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const { test } = require("node:test");

const host = fs.readFileSync(path.join(__dirname, "../webview-page/host.js"), "utf8");
function functions(first, next) {
  return host.slice(host.indexOf("  function " + first + "("), host.indexOf("  function " + next + "("));
}
function harness() {
  const nodes = Object.fromEntries([".piano-capture-state", ".piano-routing", ".capture-detail"].map(key => [key, {}]));
  const occurrence = { track: 4, chain_use: 5, phrase_use: 6 };
  const context = vm.createContext({
    model: { track: 1, tracks: [{ id: 4 }, { id: 9 }], phrase: { id: 12 },
      occurrence: { track: 9, chain_use: 10, phrase_use: 11 }, pianoControls: { pitch: 60 },
      capture: { state: { state: "armed" }, armed: { occurrence, phrase: 7 }, monitor: false } },
    pianoSource: null, stringify: JSON.stringify, Set,
    $: () => ({ querySelector: selector => nodes[selector] }),
    sameOccurrence: (a, b) => JSON.stringify(a) === JSON.stringify(b),
    beatLabel: at => `${at.numerator}/${at.denominator}`,
    value: at => at.numerator / at.denominator,
    pianoSummary: events => ({ events }),
    PianoEventIndex: class { constructor(events) { this.events = events; } },
  });
  vm.runInContext(functions("pianoContentSource", "togglePianoFollow") +
    functions("updatePianoCapture", "trackDialog") + functions("mpeZoneLabel", "pianoMetrics"), context);
  return { context, nodes };
}

test("capture preview follows its source phrase and disappears after discard", () => {
  const { context } = harness(), source = { events: [{ id: 1 }] }, other = { events: [{ id: 2 }] };
  const draft = { phrase: 7, events: [{ id: 3 }] };
  context.model.capture.draft = draft;
  assert.deepEqual(context.pianoContentSource(other).index.events, other.events);
  context.model.phrase.id = 7;
  const preview = context.pianoContentSource(source);
  assert.deepEqual(preview.index.events, [...source.events, ...draft.events]);
  assert.equal(preview.storedEvents.has(draft.events[0]), false, "draft notes cannot become editable stored notes");
  context.model.capture.draft = null;
  assert.deepEqual(context.pianoContentSource(source).index.events, source.events);
  assert.deepEqual(source.events, [{ id: 1 }], "capture preview never mutates stored music");
});

test("armed routing stays pinned while selected phrase changes", () => {
  const { context, nodes } = harness();
  context.updatePianoCapture();
  assert.match(nodes[".piano-routing"].textContent, /ARMED  Track 1 · Phrase 7.*SELECTED  Track 2 · Phrase 12/);
  assert.equal(nodes[".piano-capture-state"].textContent, "READY · OVERDUB · MONITOR OFF");
  assert.match(nodes[".capture-detail"].textContent, /Armed is not recording/);
});

test("capture transitions refresh hints and held counts without selection changes", () => {
  const { context, nodes } = harness(), capture = context.model.capture;
  capture.state = { state: "recording" }; capture.held = 2;
  context.updatePianoCapture();
  assert.match(nodes[".capture-detail"].textContent, /monitoring off.*2 held/);
  capture.monitor = true; capture.held = 0; capture.ending = { numerator: 8, denominator: 1 };
  context.updatePianoCapture();
  assert.match(nodes[".piano-capture-state"].textContent, /^ENDING TAKE/);
  assert.match(nodes[".capture-detail"].textContent, /Ending take at beat 8\/1/);
  capture.ending = null; capture.state = { state: "interrupted", reason: "Input disconnected" };
  context.updatePianoCapture();
  assert.match(nodes[".piano-routing"].textContent, /TAKE  Track 1 · Phrase 7/);
  assert.match(nodes[".capture-detail"].textContent, /Input disconnected/);
});

test("declared MIDI mode names exact lower and upper members and bend cents", () => {
  const { context } = harness();
  assert.equal(context.mpeZoneLabel({ kind: "lower", members: 15 }), "MPE LOWER · MASTER 01 · MEMBERS 02–16");
  assert.equal(context.mpeZoneLabel({ kind: "upper", members: 3 }), "MPE UPPER · MASTER 16 · MEMBERS 13–15");
  assert.equal(context.bendRangeLabel({ semitones: 2, cents: 5 }), "2.05");
});

test("recording destination displays authoritative readiness without inferring from its name", () => {
  const { context } = harness(), instrument = { name: "Braids" };
  context.model.capture.destinations = [{ instrument: 17, mpe_error: null }, { instrument: 18, mpe_error: "Graph pending" }];
  assert.match(context.recordingInstrumentLabel(17, instrument), /MPE READY$/);
  assert.match(context.recordingInstrumentLabel(18, instrument), /MPE UNAVAILABLE$/);
  assert.match(context.recordingInstrumentLabel(19, instrument), /MPE NOT VERIFIED$/);
});
