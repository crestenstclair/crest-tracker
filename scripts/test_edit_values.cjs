const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { test } = require("node:test");
const vm = require("node:vm");

// Exercise the shipped, DOM-independent editing boundary. No alternate parser.
const source = ["wire-values.js", "time-values.js", "edit-values.js"].map(file => fs.readFileSync(path.join(__dirname, "../webview-page", file), "utf8")).join("\n");
const values = vm.runInNewContext(source + "; CrestTrackerEditValues");
const plain = value => JSON.parse(JSON.stringify(value));
const beat = (numerator, denominator = 1) => ({ numerator, denominator });

test("decimal timing stays exact, including sub-microbeat values", () => {
  for (const [text, expected] of [
    ["0.0000001", beat(1, 10000000)], ["0.1234567", beat(1234567, 10000000)],
    [".125", beat(1, 8)], ["+1.2500", beat(5, 4)], ["0", beat(0)],
    [" 13 / 24 ", beat(13, 24)], ["10000000000000000/10000000000000000", beat(1)],
  ]) assert.deepEqual(plain(values.parseBeat(text)), expected, text);
  for (const text of ["", " ", "/2", "1/", "1/0", "1//2", "-1", "NaN", "Infinity", "0x10", "18446744073709551616", "1/18446744073709551616"]) {
    assert.throws(() => values.parseBeat(text), undefined, text);
  }
});

test("every editable FX round trips after JSON property reordering", () => {
  const fixtures = [
    ["offset", {kind:"offset",semitones:-12}], ["velocity", {kind:"velocity",value:88}],
    ["gate", {kind:"gate",fraction:beat(3,8)}], ["cut", {kind:"cut",after:beat(0)}],
    ["delay", {kind:"delay",beats:beat(1,10000000)}],
    ["retrigger", {kind:"retrigger",count:3,spacing:beat(1,6)}],
    ["chance", {kind:"chance",percent:65}],
    ["cc", {kind:"control",control:{kind:"cc",number:74,value:65}}],
    ["poly_pressure", {kind:"control",control:{kind:"poly_pressure",pitch:60,value:73}}],
    ["bend", {kind:"control",control:{kind:"bend",value:8193}}],
    ["pressure", {kind:"control",control:{kind:"pressure",value:73}}],
    ["program", {kind:"control",control:{kind:"program",value:0}}],
    ["tempo", {kind:"tempo",tempo:108001}], ["jump", {kind:"jump",step:7}],
    ["chord", {kind:"chord",intervals:[0,4,7]}], ["add", {kind:"add",intervals:[-12,7]}],
    ["voicing", {kind:"voicing",inversion:1,open:true}],
    ["spread", {kind:"spread",voices:2}],
    ["send", {kind:"send",bus:0,amount:65,duration:beat(1,2)}],
  ];
  const sorted = value => Array.isArray(value) ? value.map(sorted) : value && typeof value === "object"
    ? Object.fromEntries(Object.keys(value).sort().map(key => [key, sorted(value[key])])) : value;
  for (const [kind, effect] of fixtures) {
    assert.deepEqual(plain(values.parseFx(kind, values.formatFx(sorted(effect)))), effect, kind);
  }
  assert.equal(values.formatFx(sorted(fixtures.at(-1)[1])), "0,65,1/2");
  assert.equal(values.parseFx("none"), null);
  assert.deepEqual(plain(values.parseFx("bank:42")), {kind:"chord_revision",revision:42});
});

test("malformed FX fields never become zeros, booleans or ignored arguments", () => {
  for (const [kind, text] of [
    ["chance", ""], ["offset", " "], ["velocity", "88.5"], ["bend", "0x2000"],
    ["send", "0,65,1/2,99"], ["send", "0,,1/2"], ["send", "0,65"],
    ["retrigger", "3,1/6,2"], ["voicing", "1,tru"], ["voicing", "1"],
    ["cc", "74,"], ["chord", "0,4,"], ["add", ",7"],
    ["tempo", "108.0001"], ["bank:0", undefined], ["bank:1.2", undefined],
  ]) assert.throws(() => values.parseFx(kind, text), undefined, `${kind}: ${text}`);
  assert.deepEqual(plain(values.parseFx("voicing", "1, false")), {kind:"voicing",inversion:1,open:false});
});

test("Song, default and FX tempo share exact milli-BPM parsing", () => {
  for (const [text, expected] of [["123.456",123456],["108.001",108001],["0.001",1],["4294967.295",4294967295]]) {
    assert.equal(values.parseTempoMilliBpm(text), expected);
    assert.equal(values.parseFx("tempo",text).tempo, expected);
  }
  for (const text of ["0","-1","123.4567","4294967.296",""]) assert.throws(() => values.parseTempoMilliBpm(text));
});

test("new expression points retain exact shared member/master sequence ordering", () => {
  assert.equal(values.nextExpressionSequence({expression:[]}),1);
  const expression=Array.from({length:150000},(_,sequence)=>({sequence}));
  const note={expression,master_expression:[{sequence:9007199254740989}]};
  assert.equal(values.nextExpressionSequence(note),9007199254740990);
  assert.equal(expression.length,150000);
  assert.equal(expression.at(-1).sequence,149999);
  assert.equal(note.master_expression[0].sequence,9007199254740989);
  assert.equal(values.nextExpressionSequence({expression:[{sequence:42}],master_expression:[{sequence:null}]}),43);
  assert.throws(()=>values.nextExpressionSequence({expression:[{sequence:Number.MAX_SAFE_INTEGER}]}),/exhausted/);
  assert.throws(()=>values.nextExpressionSequence({expression:[{sequence:Number.MAX_SAFE_INTEGER+1}]}),/precision/);
});
