// Focused DOM contract for the dashboard opened from the widget.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync(process.env.BOSN_APP_SOURCE || 'crates/bosn-service/src/ci/ui/assets/app.js', 'utf8');

function body(name) {
  const start = source.indexOf(`function ${name}(`);
  if (start < 0) throw new Error(`missing ${name}`);
  let depth = 0;
  let begun = false;
  for (let i = source.indexOf('{', start); i < source.length; i++) {
    if (source[i] === '{') { depth++; begun = true; }
    if (source[i] === '}') depth--;
    if (begun && !depth) return source.slice(source.slice(0, start).endsWith('async ') ? start - 6 : start, i + 1);
  }
  throw new Error(`unterminated ${name}`);
}

class Node {
  constructor(name, text = '') {
    this.nodeName = name;
    this.nodeType = name === '#text' ? 3 : 1;
    this.textContent = text;
    this.nodes = [];
    this.attrs = new Map();
    this.dataset = {};
    this.isConnected = true;
    this.scrollTop = 0;
    this.clientHeight = 100;
  }
  get childNodes() { return this.nodes; }
  get children() { return this.nodes.filter(n => n.nodeType === 1); }
  get firstChild() { return this.nodes[0] || null; }
  get nextSibling() {
    if (!this.parent) return null;
    return this.parent.nodes[this.parent.nodes.indexOf(this) + 1] || null;
  }
  get scrollHeight() { return this.nodes.length * 20; }
  append(...nodes) {
    for (const node of nodes) {
      if (node.nodeName === '#fragment') this.append(...[...node.nodes]);
      else { this.nodes.push(node); node.parent = this; }
    }
  }
  insertBefore(node, before) {
    if (node.parent) node.remove();
    const index = before ? this.nodes.indexOf(before) : this.nodes.length;
    this.nodes.splice(index, 0, node);
    node.parent = this;
  }
  remove() { if (this.parent) this.parent.nodes.splice(this.parent.nodes.indexOf(this), 1); this.parent = null; }
  replaceChildren(...nodes) { this.nodes.forEach(n => { n.parent = null; }); this.nodes = []; this.append(...nodes); }
  matches(selector) { return selector === 'pre.log' && this.nodeName === 'PRE'; }
  getAttributeNames() { return [...this.attrs.keys()]; }
  hasAttribute(key) { return this.attrs.has(key); }
  getAttribute(key) { return this.attrs.get(key); }
  setAttribute(key, value) { this.attrs.set(key, value); }
  removeAttribute(key) { this.attrs.delete(key); }
}

const calls = [];
let records = [{ stream: 'stdout', text: 'first\n' }];
let pageSize = 1000;
const context = vm.createContext({
  Node: { TEXT_NODE: 3 }, URLSearchParams, WeakMap, console,
  document: { createDocumentFragment: () => new Node('#fragment') },
  el: (name, attrs, text) => new Node(name.toUpperCase(), text),
  api: async path => {
    const query = new URL(path, 'http://local').searchParams;
    const since = Number(query.get('since_seq'));
    calls.push(since);
    const next = records.slice(since, since + pageSize);
    return { records: next, next_seq: since + next.length, more: since + next.length < records.length };
  },
});
const incremental = source.includes('function reconcile(');
vm.runInContext('const logState = new WeakMap(); const logCache = new Map(); let selected = "run-1";\n' +
  (incremental ? body('reconcile') + '\n' + body('patchNode') + '\n' : '') + body('loadLog'), context);
// A quiet running setup phase must resample its elapsed duration, and stop
// polling when it finishes or the user selects another run.
const stageTimers = [];
let stageRefreshes = 0;
let stageShouldFail = false;
const stageContext = vm.createContext({
  setTimeout: (fn, delay) => { const timer = { fn, delay, cleared: false }; stageTimers.push(timer); return timer; },
  clearTimeout: (timer) => { timer.cleared = true; },
  showRun: () => { stageRefreshes++; return stageShouldFail ? Promise.reject(new Error("transient")) : Promise.resolve(); },
});
vm.runInContext('let selected = "run-1"; let stageRefresh = null;\n' + body('scheduleStageRefresh'), stageContext);
const scheduleStage = vm.runInContext('scheduleStageRefresh', stageContext);
scheduleStage({ id: 'run-1', state: 'running', stages: [{ state: 'running' }] });
assert.equal(stageTimers.length, 1);
assert.equal(stageTimers[0].delay, 1000);
stageTimers[0].fn();
assert.equal(stageRefreshes, 1);
scheduleStage({ id: 'run-1', state: 'running', stages: [{ state: 'running' }] });
scheduleStage({ id: 'run-1', state: 'done', stages: [{ state: 'succeeded' }] });
assert.equal(stageTimers[1].cleared, true);
vm.runInContext('selected = "run-2"', stageContext);
scheduleStage({ id: 'run-1', state: 'running', stages: [{ state: 'running' }] });
assert.equal(stageTimers.length, 2);

const fetchLog = (pre, run) => incremental
  ? vm.runInContext('loadLog', context)(pre, {}, run)
  : vm.runInContext('loadLog', context)(pre, 'full', {}, run);

(async () => {
  vm.runInContext('selected = "run-1"', stageContext);
  stageShouldFail = true;
  scheduleStage({ id: 'run-1', state: 'running', stages: [{ state: 'running' }] });
  const failedTimer = stageTimers.at(-1);
  failedTimer.fn();
  await new Promise(setImmediate);
  const retryTimer = stageTimers.at(-1);
  assert.notEqual(retryTimer, failedTimer, 'transient failure must rearm the stage refresh');
  assert.equal(retryTimer.delay, 1000);
  stageShouldFail = false;
  retryTimer.fn();
  await new Promise(setImmediate);

  const parent = new Node('DIV');
  const details = new Node('DETAILS');
  details.dataset.key = 'step job Build:1';
  const pre = new Node('PRE');
  details.append(pre);
  parent.append(details);
  const run = { id: 'run-1', state: 'running', log_records: 1 };
  await fetchLog(pre, run);
  assert.equal(pre.childNodes.length, 1);
  pre.scrollTop = 7;
  pre.clientHeight = 5;
  Object.defineProperty(pre, "scrollHeight", { get: () => 100 });
  const fresh = new Node('DETAILS');
  fresh.dataset.key = details.dataset.key;
  fresh.append(new Node('PRE'));
  if (incremental) vm.runInContext('reconcile', context)(parent, [fresh]);
  assert.equal(parent.firstChild, details, 'expanded details remains mounted');
  assert.equal(details.firstChild, pre, 'log node remains mounted');
  assert.equal(pre.scrollTop, 7, 'reader scroll survives status refresh');
  await fetchLog(pre, run);
  assert.deepEqual(calls, [0], 'unchanged log count needs no request');
  records.push({ stream: 'stderr', text: 'second\n' });
  run.log_records = records.length;
  await fetchLog(pre, run);
  assert.deepEqual(calls, [0, 1], 'second fetch starts after the first record');
  assert.equal(pre.childNodes.length, 2, 'prior line is retained and new line appended once');
  assert.equal(pre.scrollTop, 7, 'reader is not pulled to the bottom');
  pageSize = 1;
  records.push(...Array.from({ length: 45 }, (_, i) => ({ stream: 'stdout', text: `line ${i}\n` })));
  run.log_records = records.length;
  await fetchLog(pre, run);
  assert.equal(pre.childNodes.length, 47, 'pagination continues beyond 40 pages');
  assert.equal(pre.scrollTop, 7);
  pre.scrollTop = 95;
  records.push({ stream: 'stdout', text: 'following\n' });
  run.log_records = records.length;
  await fetchLog(pre, run);
  assert.equal(pre.scrollTop, 100, 'a reader at the bottom follows new lines');
  vm.runInContext('selected = \"run-2\"', context);
  records.push({ stream: 'stdout', text: 'stale\n' });
  await fetchLog(pre, run);
  assert.equal(pre.childNodes.length, 48, 'previous run cannot paint after selection');
  console.log('live log DOM and cursor: ok');
})().catch(error => { console.error(error); process.exitCode = 1; });
