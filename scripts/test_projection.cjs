const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const {test} = require("node:test");
const vm = require("node:vm");
const source = fs.readFileSync(path.join(__dirname, "../webview-page/projection.js"), "utf8");
const {PhraseReceiver} = vm.runInNewContext(source + "; CrestTrackerProjection");
const header = (token, revision = 1, phrase = 17) => ({token, revision, phrase});
const frame = (source, phrase) => ({revision: source.revision, phraseSource: source,
  ...(phrase === undefined ? {} : {phrase}), playing: false});

test("acknowledged phrase retains exact source through cursor/playback updates", () => {
  const receiver = new PhraseReceiver(), source = header("a");
  const phrase = {id: 17, value: {events: [{id: 9007199254740993n,
    at: {numerator: 9007199254740997n, denominator: 18014398509481984n}}]}};
  const first = receiver.receive(frame(source, phrase));
  assert.equal(first.model.phrase, phrase);
  assert.equal(first.acknowledge, "a");
  const updated = receiver.receive({...frame(source), playing: true, row: 65});
  assert.equal(updated.model.phrase, phrase);
  assert.equal(updated.model.playing, true);
  assert.equal(updated.model.row, 65);
  assert.equal(updated.acknowledge, null);
  assert.equal(updated.model.phrase.value.events[0].id, 9007199254740993n);
});

test("missing source suppresses mixed revisions and requests one resync per token", () => {
  const receiver = new PhraseReceiver();
  receiver.receive(frame(header("old"), {id: 17, value: {events: []}}));
  const changed = frame(header("new", 2));
  const missing = receiver.receive(changed);
  assert.equal(missing.model, undefined);
  assert.equal(missing.resync, true);
  assert.equal(receiver.receive(changed).resync, false);
  const recovered = receiver.receive(frame(header("recovered", 2), {id: 17, value: {events: [1]}}));
  assert.deepEqual(Array.from(recovered.model.phrase.value.events), [1]);
  assert.equal(recovered.acknowledge, "recovered");
  const reload = new PhraseReceiver();
  assert.equal(reload.receive(frame(header("recovered", 2))).resync, true);
  assert.equal(reload.receive(frame(header("recovered", 2))).resync, false);
});

test("phrase switches, no selection and content replacement remain explicit", () => {
  const receiver = new PhraseReceiver(), first = {id: 17, value: {events: [1]}};
  receiver.receive(frame(header("a"), first));
  const other = {id: 18, value: {events: [2]}};
  assert.equal(receiver.receive(frame(header("b", 1, 18), other)).model.phrase, other);
  assert.equal(receiver.receive(frame(header("a"))).model, undefined);
  assert.equal(receiver.receive(frame(header("c"), first)).model.phrase, first);
  assert.equal(receiver.receive(frame(header("empty", 2, null), null)).model.phrase, null);
  assert.equal(receiver.receive(frame(header("empty", 2, null))).model.phrase, null);
  const replaced = {id: 17, value: {events: [3]}};
  assert.equal(receiver.receive(frame(header("replace", 3), replaced)).model.phrase, replaced);
});

test("mismatched source identity or revision never reaches rendering", () => {
  const receiver = new PhraseReceiver();
  assert.equal(receiver.receive(frame(header("bad"), {id: 99})).model, undefined);
  assert.equal(receiver.receive({...frame(header("bad2"), {id: 17}), revision: 2}).model, undefined);
  receiver.receive(frame(header("ok"), {id: 17}));
  assert.equal(receiver.receive(frame(header("ok", 1, 99))).model, undefined);
});
