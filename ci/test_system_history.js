// #821: chart gaps, current status, saved preferences and superseded refreshes.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");
const source = fs.readFileSync("crates/bosn-service/src/ci/ui/assets/system.js", "utf8");
class Node {
  constructor(tag, attrs = {}) { this.tag = tag; this.attrs = attrs; this.nodes = []; this.clientWidth = 640; }
  append(...nodes) { this.nodes.push(...nodes); }
  replaceChildren(...nodes) { this.nodes = nodes; }
  setAttribute(key, value) { this.attrs[key] = value; }
  addEventListener() {}
}
const nodes = new Map();
const pending = [];
const timers = new Map();
let nextTimer = 0;
const context = vm.createContext({
  console, Date, Math, Object, Promise,
  document: { createElementNS: (_, tag) => new Node(tag), querySelectorAll: () => [] },
  $: id => { if (!nodes.has(id)) nodes.set(id, new Node(id)); return nodes.get(id); },
  el: (tag, attrs, ...children) => { const node = new Node(tag, attrs); node.append(...children); return node; },
  localStorage: { getItem: key => key === "system-range" ? "day" : "max", setItem() {} },
  setTimeout: (fn, ms) => { const id = ++nextTimer; timers.set(id, { fn, ms }); return id; },
  clearTimeout: id => timers.delete(id),
  Unauthorized: class extends Error {},
  request: path => path.startsWith("/v1/system?")
    ? new Promise(resolve => pending.push({ path, resolve }))
    : Promise.resolve({ runs: [], runners: [] }),
});
vm.runInContext(source.slice(0, source.lastIndexOf("controls();\nrefresh();")), context);
assert.equal(vm.runInContext("range", context), "day");
assert.equal(vm.runInContext("mode", context), "max");
vm.runInContext(`
  range = "hour";
  chart("test", { title: "CPU", min: 1, axis: String, show: String,
    series: [{ name: "CPU", cls: "s1", value: p => p.cpu }] }, [
      { at_ms: 0, cpu: 1 }, { at_ms: 10000, cpu: null },
      { at_ms: 20000, cpu: 2 }, { at_ms: 30000, cpu: 3 },
    ]);
`, context);
const paths = nodes.get("test").nodes[1].nodes.filter(n => n.tag === "path" && n.attrs.class === "line s1");
assert.equal(paths.length, 2, "null metric splits the path");
assert.equal(paths[0].attrs.d.includes("L"), false, "isolated point does not bridge the gap");
vm.runInContext(`
  let statusSample;
  renderStatus = (_, sample) => { statusSample = sample; };
  renderCharts = renderTree = renderTasks = renderContainers = renderDetail = () => {};
`, context);
async function main() {
  const first = vm.runInContext('range = "hour"; refresh()', context);
  const second = vm.runInContext('range = "day"; refresh()', context);
  const third = vm.runInContext('range = "hour"; refresh()', context);
  const reply = (range, cpu) => ({ range, interval_secs: range === "hour" ? 10 : 60,
    host: {}, latest: { cpu_percent: cpu }, samples: [] });
  pending[2].resolve(reply("hour", 30));
  await third;
  pending[0].resolve(reply("hour", 10));
  pending[1].resolve(reply("day", 20));
  await Promise.all([first, second]);
  assert.equal(vm.runInContext("statusSample.cpu_percent", context), 30, "current raw status ignores stale replies");
  assert.equal(timers.size, 1, "only the latest request schedules a refresh");
  assert.equal([...timers.values()][0].ms, 10000);
  vm.runInContext('mode = "avg"; updateLive()', context);
  assert.match(nodes.get("live").textContent, /average/);
  const fourth = vm.runInContext('range = "month"; refresh()', context);
  pending[3].resolve(reply("month", 40));
  await fourth;
  assert.equal(timers.size, 1);
  assert.equal([...timers.values()][0].ms, 60000);
  console.log("system history UI contracts passed");
}
main().catch(error => { console.error(error); process.exitCode = 1; });
