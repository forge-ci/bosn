// Run the shipped control scripts with delayed RFB events and idle CI.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const assets = path.resolve(__dirname, "../crates/bosn-service/src/ci/ui/assets");

class Events {
  constructor() { this.listeners = new Map(); }
  addEventListener(name, callback) {
    if (!this.listeners.has(name)) this.listeners.set(name, []);
    this.listeners.get(name).push(callback);
  }
  emit(name, event = {}) {
    for (const callback of this.listeners.get(name) || []) callback(event);
  }
}

async function reconnect() {
  const nodes = Object.fromEntries([
    "name", "status", "notice", "credentials", "password", "screen",
    "connect", "disconnect", "keys",
  ].map((id) => [id, Object.assign(new Events(), {
    textContent: "", hidden: true, value: "", disabled: false,
    focus() {}, click() { this.emit("click"); },
  })]));
  const viewers = [], channels = [];
  class RFB extends Events {
    constructor() { super(); viewers.push(this); }
    // The close event deliberately arrives after the next connection succeeds.
    disconnect() {}
    sendCredentials() {}
  }
  class WebSocket extends Events {
    constructor() { super(); channels.push(this); }
    close() {}
  }
  const source = fs.readFileSync(path.join(assets, "console.js"), "utf8")
    .replace(/^import RFB[^\n]*\n/m, "");
  await vm.runInNewContext(`(async () => {${source}\n})()`, {
    RFB, WebSocket, document: { getElementById: (id) => nodes[id] },
    location: { pathname: "/console/rack", protocol: "http:", host: "127.0.0.1:7428" },
    window: new Events(), fetch: async () => ({ ok: true,
      json: async () => [{ id: "rack", name: "Dell rack" }] }),
  });
  assert.equal(viewers.length, 1);
  viewers[0].emit("connect");
  nodes.disconnect.click();
  nodes.connect.click();
  assert.equal(viewers.length, 2);
  viewers[1].emit("connect");
  viewers[0].emit("disconnect", { detail: { clean: true } });
  viewers[0].emit("credentialsrequired");
  viewers[0].emit("securityfailure");
  channels[0].emit("message", { data: JSON.stringify({ console_error: "certificate_changed" }),
    stopImmediatePropagation() {} });
  assert.equal(nodes.status.textContent, "Connected · encrypted iDRAC VNC");
  assert.equal(nodes.disconnect.disabled, false);
  assert.equal(nodes.keys.disabled, false);
  assert.equal(nodes.credentials.hidden, true);
  assert.equal(nodes.notice.hidden, true);
  viewers[1].emit("disconnect", { detail: { clean: true } });
  assert.equal(nodes.status.textContent, "Disconnected");
  assert.equal(nodes.keys.disabled, true);
}

async function idleRefresh() {
  let servers = [{ id: "first", name: "First Dell" }];
  const box = { children: [], replaceChildren(...nodes) { this.children = nodes; },
    append(node) { this.children.push(node); } };
  const timers = [];
  const source = fs.readFileSync(path.join(assets, "widget.js"), "utf8");
  const section = source.slice(source.indexOf("// Server configuration is independent"));
  await vm.runInNewContext(`let consoleRefreshPending = false; (async () => {${section}\n})()`, {
    $: () => box, api: async () => servers, act() {},
    el: (tag, attributes, ...children) => ({ tag, attributes, children }),
    setInterval: (callback, milliseconds) => timers.push({ callback, milliseconds }),
  });
  await new Promise(setImmediate);
  assert.equal(box.children[0].children[0].children[0], "First Dell");
  assert.equal(timers.length, 1, "idle CI must still schedule server configuration refresh");
  assert.equal(timers[0].milliseconds, 10000);
  servers = [{ id: "second", name: "Second Dell" }];
  await timers[0].callback();
  assert.equal(box.children[0].children[0].children[0], "Second Dell");
}

(async () => {
  await reconnect();
  await idleRefresh();
  console.log("console controls: stale events ignored; idle server refresh passed");
})().catch((error) => { console.error(error); process.exitCode = 1; });
