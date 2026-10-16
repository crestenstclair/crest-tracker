const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const { test } = require('node:test');
const host = fs.readFileSync(path.join(__dirname, '../webview-page/host.js'), 'utf8');
const functions = (first, next) => host.slice(host.indexOf(`  function ${first}(`), host.indexOf(`  function ${next}(`));

function harness() {
  const document = { activeElement: null };
  class Element {
    constructor(tag = 'div') { this.tagName = tag.toUpperCase(); this.children = []; this.dataset = {}; this.style = {}; this.hidden = false; }
    append(...nodes) { for (const node of nodes) { node.parentElement = this; this.children.push(node); } }
    replaceChildren(...nodes) { this.children = []; this.append(...nodes); }
    querySelectorAll(selector) {
      const tags = selector.split(',').map(part => part.trim().toUpperCase());
      return this.children.flatMap(node => [...(tags.includes(node.tagName) ? [node] : []), ...node.querySelectorAll(selector)]);
    }
    querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
    getClientRects() { return this.hidden || this.parentElement && !this.parentElement.getClientRects().length ? [] : [{}]; }
    focus() { document.activeElement = this; }
    scrollIntoView() {}
    click() { this.onclick?.(); }
    dispatchEvent(event) { this[`on${event.type}`]?.(event); }
    get options() { return this.children; }
    get selectedOptions() { return this.children.filter(node => node.value === this.value); }
  }
  const fields = new Element(), footer = new Element(), dialog = new Element(), form = new Element();
  const nodes = { 'tracker-fields': fields, 'tracker-dialog': dialog, 'tracker-form': form,
    'tracker-actions': new Element(), 'tracker-settings': new Element() };
  form.querySelector = selector => selector === 'footer' ? footer : null;
  form.querySelectorAll = () => [];
  const calls = [];
  const context = vm.createContext({ document, Event: class { constructor(type) { this.type = type; } },
    model: { view: 'song' }, controllerFocus: null, controllerText: null, controllerSurface: null, activeFileDialog: null,
    $: id => nodes[id], queueMicrotask: run => run(),
    button: (label, run) => { const button = new Element('button'); button.textContent = label; button.onclick = run; return button; },
    form: title => { dialog.open = true; dialog.title = title; },
    hideDialog: () => { dialog.open = false; }, closeDialog: () => { dialog.open = false; },
    goTo: destination => calls.push(destination), emit: request => calls.push(request), request: kind => calls.push(kind),
    file: action => calls.push(action), chordBankDialog: () => calls.push('chords'), dialogNote: () => {},
    destinations: [{ value: 'song', label: 'Tracker · Song' }, { value: 'sends', label: 'Sends' }, { value: 'settings', label: 'Settings' }],
  });
  vm.runInContext(functions('surfaceControls', 'openCharacterChooser') + functions('navigationMenu', 'contextEdit'), context);
  return { context, document, nodes, fields, calls, Element };
}

test('Main menu has documented destinations; Up/Down and Confirm select directly', () => {
  const { context, document, fields, calls } = harness();
  context.controllerAction('menu');
  assert.deepEqual(fields.children.map(node => node.textContent), ['Tracker', 'Instruments', 'Chord Bank', 'Mixer', 'Sends', 'Settings']);
  assert.equal(document.activeElement.textContent, 'Tracker');
  for (let index = 0; index < 3; index++) context.controllerAction('next');
  assert.equal(document.activeElement.textContent, 'Mixer');
  context.controllerAction('confirm');
  assert.deepEqual(calls, ['mixer']);
});

test('Quick Jump includes Song File, Sends and Settings; Back preserves editing context', () => {
  const { context, nodes, fields, calls } = harness();
  context.model.view = 'phrase';
  context.controllerAction('quick_jump');
  assert.deepEqual(fields.children.map(node => node.textContent), ['Tracker · Song', 'Sends', 'Settings', 'Song File']);
  context.controllerAction('back');
  assert.equal(nodes['tracker-dialog'].open, false);
  assert.equal(context.model.view, 'phrase');
  assert.deepEqual(calls, []);
  context.songFileMenu();
  context.controllerAction('shift_up');
  assert.equal(calls[0].kind, 'view');
  assert.equal(calls[0].view, 'settings');
});

test('controller focuses actual visible toolbar controls and returns without editing', () => {
  const { context, nodes, calls, Element, document } = harness();
  const first = new Element('button'), hidden = new Element('button'), second = new Element('button');
  hidden.hidden = true; second.onclick = () => calls.push('opened');
  nodes['tracker-actions'].append(first, hidden, second);
  context.controllerAction('focus_controls');
  assert.equal(document.activeElement, first);
  context.controllerAction('next');
  assert.equal(document.activeElement, second);
  context.controllerAction('back');
  assert.equal(context.controllerSurface, null);
  assert.deepEqual(calls.map(call => call.active), [true, false]);
  context.controllerAction('focus_controls');
  context.controllerAction('next');
  context.controllerAction('confirm');
  assert.equal(calls.at(-1), 'opened');
});

test('FX picker browses commands vertically; Confirm advances to value or Save', () => {
  const { context, nodes, fields, document, Element } = harness();
  const command = new Element('select'), value = new Element('input');
  command.dataset.controllerBrowse = 'true';
  for (const name of ['none', 'offset', 'send']) { const option = new Element('option'); option.value = name; command.append(option); }
  command.value = 'none'; fields.append(command, value); command.focus(); nodes['tracker-dialog'].open = true;
  context.controllerAction('next'); context.controllerAction('next');
  assert.equal(command.value, 'send'); assert.equal(document.activeElement, command);
  context.controllerAction('confirm'); assert.equal(document.activeElement, value);
});

test('focused FX editor changes only its originating slot and preserves exact note/control content', () => {
  const original = { id: 9007199254740993n, at: { numerator: 13, denominator: 24 }, target: { kind: 'channel', channel: 1, endpoint: null },
    note: { pitch: 60, velocity: 101, release_velocity: 33, duration: { numerator: 5, denominator: 7 }, expression: [{ raw: 8193 }] },
    controls: [{ controller: 64, value: 127 }], fx: [{ kind: 'offset', semitones: 3 }, null, { kind: 'velocity', value: 87 }] };
  let submit; const calls = [], command = { dataset: {}, value: 'none' }, value = { parentElement: { firstChild: {} } };
  const context = vm.createContext({
    model: { phrase: { id: 7 }, revision: 18 }, fxTypes: ['none', 'send'], phraseEventDraft: () => structuredClone(original),
    structuredClone, fxChoice: fx => fx?.kind || 'none', fxValue: () => '', fxValueLabel: kind => kind,
    parseFx: () => ({ kind: 'send', bus: 0, amount: 65, duration: { numerator: 1, denominator: 2 } }),
    form: (_title, _fields, apply) => { submit = apply; }, emit: request => calls.push(request), focusController: () => {},
    $: () => ({ querySelector: selector => selector === 'select' ? command : value }),
  });
  vm.runInContext(functions('fxDialog', 'pianoControlsDialog'), context);
  context.fxDialog(1);
  assert.deepEqual(calls, [], 'opening or browsing never edits shared content');
  submit({ command: 'send', value: '0,65,1/2' });
  const applied = calls[0];
  assert.equal(applied.revision, 18); assert.equal(applied.command.phrase, 7);
  const expected = structuredClone(original); expected.fx[1] = { kind: 'send', bus: 0, amount: 65, duration: { numerator: 1, denominator: 2 } };
  assert.deepEqual(applied.command.event, expected);
});

test('Settings keyboard navigation has one owner; toolbar capture uses browser handler', () => {
  const { context, document, nodes } = harness();
  let listener; const actions = [];
  context.controllerAction = action => actions.push(action);
  document.addEventListener = (_name, callback) => { listener = callback; };
  const start = host.indexOf('  document.addEventListener("keydown",event=>{\n    if($("tracker-dialog").open)');
  const end = host.indexOf('  document.addEventListener("keydown",event=>{', start + 1);
  vm.runInContext(host.slice(start, end), context);
  context.model = { view: 'settings', controlsFocused: false };
  context.controllerSurface = { view: 'settings' };
  nodes['tracker-dialog'].open = false;
  const key = { key: 's', preventDefault() {}, stopPropagation() {} };
  listener(key);
  assert.deepEqual(actions, [], 'native Settings input must not be handled a second time in the browser');
  context.model.controlsFocused = true;
  listener(key);
  assert.deepEqual(actions, ['next']);
});
