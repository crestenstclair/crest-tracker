const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { test } = require("node:test");
const vm = require("node:vm");
const source = ["wire-values.js", "time-values.js", "edit-values.js", "phrase-view.js"]
  .map(file => fs.readFileSync(path.join(__dirname, "../webview-page", file), "utf8")).join("\n");
const {wire, time, edit, view} = vm.runInNewContext(source + "; ({wire:CrestTrackerWireValues,time:CrestTrackerTimeValues,edit:CrestTrackerEditValues,view:CrestTrackerPhraseView})");

test("browser JSON and whole-phrase edits preserve u64 times, IDs and seeds", () => {
  const native = '{"name":"9007199254740993","form":{"seed":{"$crestU64":"18446744073709551615"}},"events":[{"id":{"$crestU64":"9007199254740993"},"at":{"numerator":{"$crestU64":"9007199254740993"},"denominator":{"$crestU64":"36028797018963968"}}}]}';
  const phrase = wire.decode(JSON.parse(native));
  assert.equal(phrase.name, "9007199254740993");
  assert.equal(phrase.events[0].id, 9007199254740993n);
  assert.equal(phrase.form.seed, 18446744073709551615n);
  assert.equal(time.beatLabel(phrase.events[0].at), "9007199254740993/36028797018963968");
  const renamed = structuredClone(phrase);
  renamed.name = "Renamed only";
  const sent = JSON.parse(JSON.stringify(wire.encode(renamed)));
  assert.deepEqual(sent, {...JSON.parse(native), name: "Renamed only"});
  assert.equal(wire.stringify(renamed), JSON.stringify(sent));
  const groups = {$crestU64: {selections: []}};
  assert.equal(wire.decode(groups), groups, "user-named maps are not integer tags");
});

test("full u64 fractions remain exact through text, scaling, FX and Drum membership", () => {
  const at = time.parseBeat("9007199254740993/36028797018963968");
  assert.equal(time.compareBeats(at, time.parseBeat("1/4")), 1);
  assert.equal(time.beatLabel(time.parseBeat("18446744073709551615")), "18446744073709551615");
  assert.equal(time.beatLabel(time.scaleBeat(at, 2)), "9007199254740993/18014398509481984");
  assert.equal(time.beatLabel(time.scaleBeat(time.parseBeat("1/18014398509481984"), 9, 10)), "9/180143985094819840");
  assert.equal(edit.formatFx(edit.parseFx("delay", time.beatLabel(at))), time.beatLabel(at));
  assert.equal(edit.parseFx("bank:9007199254740993").revision, 9007199254740993n);
  const window = view.drumWindow([{id: 1, at}, {id: 2, at: time.parseBeat("1/4")}], time.parseBeat("1/4"), {start: 0, end: 4});
  assert.equal(window.offgrid[0].id, 1);
  assert.equal(window.steps.get(1)[0].id, 2);
});

test("integer boundary rejects invalid or rounded values without coercing text", () => {
  for (const value of ["", "01", "-1", "1.0", "1e3", "18446744073709551616", 9007199254740992, NaN, Infinity]) {
    assert.throws(() => wire.unsignedInteger(value), undefined, String(value));
  }
  for (const value of ["18446744073709551616", "1/18446744073709551616", "1/0"]) assert.throws(() => time.parseBeat(value));
  assert.throws(() => wire.encode({seed: -1n}));
  assert.equal(wire.unsignedInteger("9007199254740991"), Number.MAX_SAFE_INTEGER);
  assert.equal(wire.unsignedInteger("9007199254740992"), 9007199254740992n);
});
