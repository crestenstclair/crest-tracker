const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { test } = require("node:test");
const vm = require("node:vm");

const source = ["wire-values.js", "time-values.js", "phrase-view.js"].map(file => fs.readFileSync(path.join(__dirname, "../webview-page", file), "utf8")).join("\n");
const view = vm.runInNewContext(source + "; CrestTrackerPhraseView");
const beat = (numerator, denominator = 1) => ({ numerator, denominator });
const event = (id, at, pitch = 60) => ({ id, at, note: { pitch }, controls: [] });
const ids = events => Array.from(events, event => event.id);

test("Drum hits match native exact toggles; near-grid notes stay off-grid", () => {
  const events = [event(1, beat(0)), event(2, beat(1, 10000000000)),
    event(3, beat(1, 4)), event(4, beat(500000001, 2000000000)),
    event(5, beat(2, 8)), event(6, beat(1)), event(7, beat(9999999999, 10000000000))];
  const window = view.drumWindow(events, beat(1, 4), { start: 0, end: 4 });
  assert.deepEqual(ids(window.steps.get(0)), [1]);
  assert.deepEqual(ids(window.steps.get(1)), [3, 5]);
  assert.deepEqual(ids(window.offgrid), [2, 4, 7]);
  assert.equal(window.steps.has(4), false, "end is exclusive");
  const next = view.drumWindow(events, beat(1, 4), { start: 4, end: 8 });
  assert.deepEqual(ids(next.steps.get(4)), [6]);
  assert.equal(next.offgrid.length, 0);
});

test("Drum tuplet windows preserve absolute steps, overlaps and source identity", () => {
  const events = [event(1, beat(63, 6)), event(2, beat(64, 6)),
    event(3, beat(32, 3)), event(4, beat(65, 6)), event(5, beat(129, 12)), event(6, beat(128, 6))];
  const before = JSON.stringify(events);
  const window = view.drumWindow(events, beat(1, 6), { start: 64, end: 128 });
  assert.deepEqual(ids(window.steps.get(64)), [2, 3]);
  assert.deepEqual(ids(window.steps.get(65)), [4]);
  assert.deepEqual(ids(window.offgrid), [5]);
  assert.equal(window.steps.get(64)[0], events[1]);
  assert.equal(JSON.stringify(events), before);
});

test("large Piano Roll summaries avoid argument limits and retain every event", () => {
  const events = Array.from({length: 150000}, (_, id) => event(id, beat(id, 4), id % 128));
  events[42].controls.push({kind: "pressure", value: 73});
  events.push({id: 150000, at: beat(0), note: null, controls: [{kind:"bend",value:8193}]});
  const summary = view.pianoSummary(events, 60);
  assert.equal(summary.low, 0);
  assert.equal(summary.high, 127);
  assert.equal(summary.notes.length, 150000);
  assert.deepEqual(ids(summary.controls), [42, 150000]);
  assert.equal(summary.notes.at(-1), events[149999]);
  const empty = view.pianoSummary([], 96);
  assert.equal(empty.low, 60);
  assert.equal(empty.high, 96);
});

test("event pages expose each source event exactly once and clamp after deletion", () => {
  const events = Array.from({length: 130}, (_, id) => event(id, beat(0)));
  const pages = [0, 64, 128].map(start => view.pageItems(events, start));
  assert.deepEqual(pages.map(page => page.values.length), [64, 64, 2]);
  assert.deepEqual(pages.flatMap(page => ids(page.values)), ids(events));
  assert.equal(pages[2].values[1], events[129]);
  assert.equal(view.pageItems(events, 64).start, 64);
  assert.equal(view.pageItems(events.slice(0, 64), 128).start, 0);
  assert.equal(view.pageItems([], 128).values.length, 0);
  assert.equal(view.pageItems(events, -64).start, 0);
});

const noteEvent = (id, at, duration, pitch = 60) => ({
  ...event(id, at, pitch), note: { pitch, duration },
});

const windowAt = (total, anchor = beat(0), zoom = 1) => new view.PianoTimeWindow({total,anchor,viewportWidth:1664,zoom});
const allPitches = {left:0,right:Infinity,low:0,high:127};
const visibleIds = entries => ids(entries.map(entry=>entry.event));

test("Piano viewport retains sustained overlaps, tiny notes, controls and source order", () => {
  const events = [
    noteEvent(1, beat(0), beat(20)), noteEvent(2, beat(11), beat(1)),
    noteEvent(3, beat(9), beat(2)), noteEvent(4, beat(10), beat(1,10000000000)),
    noteEvent(5, beat(10), beat(1), 61),
    {id:6,at:beat(10),note:null,controls:[{kind:"bend",value:8193}]},
    noteEvent(9007199254740993n, beat(100000000001n,10000000000n), beat(1)),
  ];
  events[4].controls.push({kind:"cc",number:74,value:72});
  const before=events.map(item=>item.at),index=new view.PianoEventIndex(events),window=windowAt(beat(32));
  const bounds={left:1065,right:1164,low:60,high:60},notes=index.notesIn(bounds,window);
  assert.deepEqual(visibleIds(notes),[1,3,4,9007199254740993n]);
  assert.equal(notes[2].width,4,"subpixel notes remain clickable");
  assert.equal(notes[3].event,events[6]);
  assert.deepEqual(visibleIds(index.controlsIn(bounds,window)),[5,6]);
  assert.equal(index.notesIn({...bounds,low:62,high:70},window).length,0);
  assert.deepEqual(events.map(item=>item.at),before);
});

test("bounded sections expose all 8193 notes and retain long tails across sections", () => {
  const events=Array.from({length:8192},(_,id)=>noteEvent(id,beat(id,4),beat(1,8),id%24+48));
  const sustained=noteEvent(9007199254740993n,beat(0),beat(2049),60);events.push(sustained);
  const index=new view.PianoEventIndex(events),seen=new Set();
  for(let at=0;at<2048;at+=64){
    const window=windowAt(beat(2048),beat(at+32));
    const visible=index.notesIn(allPitches,window);
    assert.equal(visible.length,257);
    assert.equal(visible.at(-1).event,sustained);
    assert.equal(visible.at(-1).left,64);
    assert.equal(visible.at(-1).width,window.width);
    assert.ok(window.width<=6400,"pixel width stays independent of phrase length");
    for(const entry of visible)seen.add(entry.event.id);
    const firstScreen=index.notesIn({...allPitches,right:1664},window);
    assert.equal(firstScreen.length,65);
  }
  assert.equal(seen.size,8193);
  assert.equal(sustained.note.duration.numerator,2049);
});

test("Piano index keeps capture drafts distinct from committed event IDs", () => {
  const stored=noteEvent(1,beat(0),beat(64)),draft=noteEvent(1,beat(60),beat(16));
  const index=new view.PianoEventIndex([stored,draft]),window=windowAt(beat(68),beat(96));
  const visible=index.notesIn(allPitches,window);
  assert.equal(visible.length,1,"a release exactly at section start is excluded");
  assert.equal(visible[0].event,draft);
  assert.equal(visible[0].width,400);
  const overlap=index.notesIn(allPitches,windowAt(beat(68)));
  assert.equal(overlap.length,2);
  assert.equal(overlap[0].event,stored);
  assert.equal(overlap[1].event,draft);
});

test("Piano render entries cannot expose or corrupt cached interval endpoints", () => {
  const sustained = noteEvent(1, beat(0), beat(20));
  const raw = { id: 2, at: beat(12), note: null, controls: [{ kind: "pressure", value: 72 }] };
  const index = new view.PianoEventIndex([sustained, raw]), window = windowAt(beat(32));
  const bounds = { left: 1064, right: 1364, low: 60, high: 60 };
  for (const entry of [...index.notesIn(bounds, window), ...index.controlsIn(bounds, window)]) {
    for (const field of ["start", "end", "furthestEnd"]) {
      assert.equal(Object.hasOwn(entry, field), false, "internal interval cache stays private: " + field);
    }
    entry.left = -999;
    entry.right = -998;
  }
  assert.deepEqual(visibleIds(index.notesIn(bounds, window)), [1]);
  assert.deepEqual(visibleIds(index.controlsIn(bounds, window)), [2]);
  assert.equal(sustained.note.duration.numerator, 20);
});

test("dense pitch rows keep every simultaneous note in stable bounded groups", () => {
  const events=Array.from({length:8193},(_,id)=>noteEvent(id,beat(0),beat(1,8),60));
  const index=new view.PianoEventIndex(events),window=windowAt(beat(64));
  const all=index.notesIn(allPitches,window),counts=new Map();
  assert.equal(all.length,events.length);
  for(const entry of all)counts.set(entry.group,(counts.get(entry.group)||0)+1);
  assert.equal(Math.max(...counts.values()),view.PIANO_NOTE_GROUP_SIZE);
});

test("exact huge time windows separate nearby notes and snap without absolute float conversion", () => {
  const base=18446744073709551552n,total=beat(18446744073709551615n);
  const events=[noteEvent(1,beat(base+1n),beat(1,8)),noteEvent(2,beat(base+2n),beat(1,8)),
    noteEvent(3,beat(base-1n),beat(3)),noteEvent(4,beat(base-1n),beat(1)),
    {id:5,at:beat(base),note:null,controls:[{kind:"pressure",value:72}]}];
  const window=windowAt(total,beat(base+32n)),index=new view.PianoEventIndex(events);
  assert.equal(window.start.numerator,base);
  assert.equal(window.width,6300);
  const visible=index.notesIn(allPitches,window);
  assert.deepEqual(visibleIds(visible),[1,2,3]);
  assert.deepEqual(Array.from(visible,entry=>entry.left),[164,264,64]);
  assert.deepEqual(visibleIds(index.controlsIn(allPitches,window)),[5]);
  assert.equal(BigInt(window.editGridBeatAtPixel(264,beat(1,4)).numerator),base+2n);
  assert.throws(()=>window.editGridBeatAtPixel(239,beat(1,4)),/Integer exceeds/,"unrepresentable derived time must reject rather than round");
});

test("section navigation and ticks stay within exact short and final phrase bounds", () => {
  const short=windowAt(beat(1,10000000));
  assert.equal(short.width,1600);
  const last=windowAt(beat(129,2),beat(999));
  assert.equal(last.start.numerator,64n);
  assert.equal(last.width,50);
  assert.equal(last.contains(beat(129,2)),false);
  assert.equal(last.adjacentSectionAnchor(-1).numerator,64n);
  assert.equal(windowAt(beat(80)).adjacentSectionAnchor(-1).numerator,0n);
  const ticks=last.ticks(beat(1,6));
  assert.equal(ticks[0].left,64);
  assert.equal(ticks.length,4);
  assert.ok(short.ticks(beat(1,18446744073709551615n)).length<=257);
  const at=last.editGridBeatAtPixel(100000,beat(1,4));
  assert.equal(Number(at.numerator)/Number(at.denominator),64.25);
});

test("owned expression preserves held value and clips beyond section without mutating points", () => {
  const event=noteEvent(1,beat(60),beat(16));
  const curve=[{offset:beat(0),control:{value:8192}},{offset:beat(2),control:{value:9000}},
    {offset:beat(5),control:{value:10000}},{offset:beat(10),control:{value:11000}}];
  const before=JSON.stringify(curve),window=windowAt(beat(68),beat(96));
  const points=view.pianoExpressionPoints(event,curve,window,c=>c.value);
  assert.deepEqual(Array.from(points,p=>[p.x,p.value]),[[64,9000],[164,10000]]);
  assert.equal(JSON.stringify(curve),before);
});

test("overlapping sections retain context when a cursor reverses after crossing", () => {
  const before=windowAt(beat(2048),beat(0));
  assert.equal(before.contains(beat(255,4)),true);
  assert.equal(before.contains(beat(64)),false);
  const after=windowAt(beat(2048),beat(64));
  assert.equal(after.start.numerator,32n);
  assert.equal(after.contains(beat(255,4)),true);
  assert.equal(after.contains(beat(64)),true);
  const next=windowAt(beat(2048),after.adjacentSectionAnchor(1));
  assert.equal(next.start.numerator,64n);
  const previous=windowAt(beat(2048),after.adjacentSectionAnchor(-1));
  assert.equal(previous.start.numerator,0n);
});

test("exact interval index reaches the end of a 150000-event phrase", () => {
  const events=Array.from({length:150000},(_,id)=>noteEvent(id,beat(id),beat(1,2),id%128));
  const index=new view.PianoEventIndex(events),window=windowAt(beat(150000),beat(149984));
  const bounds={left:0,right:Infinity,low:60,high:72};
  const visible=index.notesIn(bounds,window);
  const expected=events.slice(149952).filter(item=>item.note.pitch>=60&&item.note.pitch<=72);
  assert.deepEqual(visibleIds(visible),ids(expected));
  assert.equal(visible[0].event,expected[0]);
  assert.equal(index.notesIn(allPitches,window).length,48);
});
