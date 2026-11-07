// bosn system monitor: host status and one hour of charts from /v1/system, and
// recent CI runs from /v1/runs. Read-only. Helpers come from /shared.js.
"use strict";

const REFRESH_MS = 10000;
const SVG = "http://www.w3.org/2000/svg";
const GIB = 1024 ** 3;
const VIEW = { w: 620, h: 190, left: 60, right: 8, top: 8, bottom: 22 };
let workspace = null; // tree selection: null means the whole node
let lastRuns = [];
let lastRunners = null;
let lastCapacity = null; // /v1/runners: the CI engine's limits, or why there are none
let lastHost = null;
let selected = null; // the container whose details are open, by id

function svg(tag, attrs = {}, ...children) {
  const node = document.createElementNS(SVG, tag);
  for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, v);
  node.append(...children);
  return node;
}

const clock = (ms) => new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
const stamp = (secs) => (secs == null ? "–" : new Date(secs * 1000).toLocaleString([], {
  month: "short", day: "2-digit", hour: "2-digit", minute: "2-digit", second: "2-digit",
}));
const known = (value, show) => (value == null ? "unavailable" : show(value));

function uptime(secs) {
  const d = Math.floor(secs / 86400);
  const h = Math.floor(secs / 3600) % 24;
  const m = Math.floor(secs / 60) % 60;
  return `${d} days ${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}`;
}

// The smallest 1/2/5 step that gives at most four intervals up to `max`.
function niceStep(max) {
  const magnitude = 10 ** Math.floor(Math.log10(max / 4));
  return [1, 2, 5, 10].map((m) => m * magnitude).find((s) => max / s <= 4);
}

function chart(id, spec, samples) {
  // Draw at the panel's real width so text is never stretched.
  VIEW.w = Math.max(280, $(id).clientWidth - 26);
  const series = spec.series.map((s) => ({ ...s, points: samples.filter((p) => s.value(p) != null) }));
  const head = el("div", { class: "chart-head" }, el("h2", {}, spec.title));
  if (series.length > 1) {
    head.append(el("div", { class: "legend" }, ...series.map((s) => el("span", {},
      el("span", { class: `swatch ${s.cls}` }), s.name))));
  }
  if (!series.some((s) => s.points.length > 1)) {
    // CPU needs two readings for its first point, so one early point or none is not yet a verdict.
    const reported = series.some((s) => s.points.length > 0) || samples.length < 3;
    $(id).replaceChildren(head, el("p", { class: "empty" }, reported ? "Collecting samples…" : "Not reported on this platform."));
    return;
  }
  const t0 = samples[0].at_ms;
  const t1 = samples[samples.length - 1].at_ms;
  const peak = Math.max(spec.min, ...series.flatMap((s) => s.points.map(s.value)));
  const step = niceStep(spec.max ?? peak * 1.05);
  const top = spec.max ?? Math.ceil(peak * 1.05 / step) * step;
  const x = (t) => VIEW.left + (t - t0) / Math.max(1, t1 - t0) * (VIEW.w - VIEW.left - VIEW.right);
  const y = (v) => VIEW.h - VIEW.bottom - v / top * (VIEW.h - VIEW.top - VIEW.bottom);
  const root = svg("svg", {
    class: "chart", viewBox: `0 0 ${VIEW.w} ${VIEW.h}`,
    role: "img", "aria-label": `${spec.title}, last hour`,
  });
  for (let v = 0; v <= top + step / 1000; v += step) {
    root.append(svg("line", { class: "gridline", x1: VIEW.left, x2: VIEW.w - VIEW.right, y1: y(v), y2: y(v) }));
    const label = svg("text", { x: VIEW.left - 6, y: y(v) + 4, "text-anchor": "end" });
    label.textContent = spec.axis(v);
    root.append(label);
  }
  for (const f of [0, 0.5, 1]) {
    const label = svg("text", { x: VIEW.left + f * (VIEW.w - VIEW.left - VIEW.right), y: VIEW.h - 6, "text-anchor": ["start", "middle", "end"][f * 2] });
    label.textContent = clock(t0 + f * (t1 - t0));
    root.append(label);
  }
  for (const s of series) {
    if (s.points.length < 2) continue;
    const coords = s.points.map((p) => `${x(p.at_ms).toFixed(1)},${y(s.value(p)).toFixed(1)}`);
    const base = y(0).toFixed(1);
    const area = `M${x(s.points[0].at_ms).toFixed(1)},${base}L${coords.join("L")}L${x(s.points[s.points.length - 1].at_ms).toFixed(1)},${base}Z`;
    root.append(svg("path", { class: `area ${s.cls}`, d: area }), svg("path", { class: `line ${s.cls}`, d: `M${coords.join("L")}` }));
  }
  const cross = svg("line", { class: "cross", y1: VIEW.top, y2: VIEW.h - VIEW.bottom, visibility: "hidden" });
  root.append(cross);
  const tip = $("tip");
  root.addEventListener("pointermove", (event) => {
    const box = root.getBoundingClientRect();
    const at = t0 + ((event.clientX - box.left) / box.width * VIEW.w - VIEW.left) / (VIEW.w - VIEW.left - VIEW.right) * (t1 - t0);
    const near = samples.reduce((a, b) => (Math.abs(b.at_ms - at) < Math.abs(a.at_ms - at) ? b : a));
    cross.setAttribute("x1", x(near.at_ms));
    cross.setAttribute("x2", x(near.at_ms));
    cross.setAttribute("visibility", "visible");
    tip.textContent = [new Date(near.at_ms).toLocaleTimeString(),
      ...series.map((s) => `${s.name}: ${s.value(near) == null ? "–" : spec.show(s.value(near))}`)].join("\n");
    tip.hidden = false;
    tip.style.left = `${Math.min(event.clientX + 12, innerWidth - tip.offsetWidth - 8)}px`;
    tip.style.top = `${event.clientY + 12}px`;
  });
  root.addEventListener("pointerleave", () => {
    tip.hidden = true;
    cross.setAttribute("visibility", "hidden");
  });
  $(id).replaceChildren(head, root);
}

function meter(label, value, fraction) {
  const fill = el("span", { class: "bar-fill" });
  fill.style.width = `${Math.min(100, (fraction ?? 0) * 100).toFixed(1)}%`;
  return el("div", { class: "meter" }, el("span", {}, label), el("span", { class: "meter-value" }, value), el("span", { class: "bar" }, fill));
}

function usage(used, total) {
  if (total === 0) return ["none configured", null];
  if (used == null || !total) return ["unavailable", null];
  return [`${(used / total * 100).toFixed(2)}% (${humanBytes(used)} of ${humanBytes(total)})`, used / total];
}

function engineText(reply) {
  if (!reply) return "–";
  const engine = reply.engine_capacity;
  if (!engine) return `unavailable: ${reply.engine_capacity_error || "no reason reported"}`;
  const memory = (engine.memory_bytes / GIB).toFixed(0);
  return `${engine.running ? "running" : "next"}: ${engine.cpus} CPUs, ${memory} GiB`;
}

function renderStatus(host, sample) {
  const cpu = sample?.cpu_percent;
  $("title").textContent = `Node '${host.hostname || "local"}'`;
  $("status-title").textContent = `${host.hostname || "local"} (Uptime: ${known(host.uptime_secs, uptime)})`;
  $("status").replaceChildren(
    meter("CPU usage", cpu == null ? "collecting…" : `${cpu.toFixed(2)}% of ${host.logical_cpus} CPUs`, cpu == null ? null : cpu / 100),
    meter("RAM usage", ...usage(sample?.mem_used_bytes, host.mem_total_bytes)),
    meter("Disk space", ...usage(sample?.disk_used_bytes, host.disk_total_bytes)),
    meter("Swap usage", ...usage(sample?.swap_used_bytes, host.swap_total_bytes)),
  );
  const r = lastRunners;
  $("facts").replaceChildren(...[
    ["Load average", known(host.load, (l) => l.join(", "))],
    ["IO delay", known(sample?.io_delay_percent, (v) => `${v.toFixed(2)}%`)],
    ["CPU(s)", `${host.logical_cpus} x ${host.cpu_model || "unknown"}`],
    ["Kernel", host.kernel || "unavailable"],
    ["Disk measured at", host.disk_path],
    ["CI runners", r ? runnersText(r) : "–"],
    ["CI engine", engineText(lastCapacity)],
    ["Other containers", known(host.other_running_containers, (n) => `${n} running outside bosn`)],
  ].flatMap(([k, v]) => [el("dt", {}, k), el("dd", {}, v)]));
}

function renderCharts(host, samples) {
  // Replacing a hovered chart removes it without a pointerleave.
  $("tip").hidden = true;
  const percent = (v) => `${v.toFixed(2)}%`;
  const gib = (v) => (v >= 1024 ? `${+(v / 1024).toFixed(1)} TiB` : `${+v.toFixed(1)} GiB`);
  const inGib = (field) => (p) => (p[field] == null ? null : p[field] / GIB);
  const single = (name, value) => [{ name, cls: "s1", value }];
  chart("chart-cpu", {
    title: "CPU usage", min: 1, max: 100, axis: (v) => `${v}%`, show: percent,
    series: [{ name: "CPU usage", cls: "s1", value: (p) => p.cpu_percent }, { name: "IO delay", cls: "s2", value: (p) => p.io_delay_percent }],
  }, samples);
  chart("chart-load", {
    title: "Server load", min: 1, axis: (v) => `${+v.toFixed(2)}`, show: (v) => v.toFixed(2),
    series: single("1-minute load", (p) => p.load1),
  }, samples);
  chart("chart-memory", {
    title: "Memory usage", min: 1, max: host.mem_total_bytes ? host.mem_total_bytes / GIB : undefined, axis: gib, show: (v) => `${v.toFixed(2)} GiB`,
    series: single("Used", inGib("mem_used_bytes")),
  }, samples);
  chart("chart-disk", {
    title: "Disk usage", min: 1, max: host.disk_total_bytes ? host.disk_total_bytes / GIB : undefined, axis: gib, show: (v) => `${v.toFixed(2)} GiB`,
    series: single("Used", inGib("disk_used_bytes")),
  }, samples);
}

function treeButton(label, depth, count, value) {
  return el("li", {}, el("button", {
    type: "button", class: `depth-${depth}`, "aria-current": String(workspace === value),
    onclick: () => { workspace = value; renderTree(); renderTasks(); },
  }, el("span", { class: "label", title: label }, label), count == null ? "" : el("span", { class: "meta" }, String(count))));
}

function renderTree() {
  const counts = new Map();
  for (const run of lastRuns) counts.set(run.workspace, (counts.get(run.workspace) || 0) + 1);
  const shortName = (path) => path.split(/[\\/]/).filter(Boolean).pop() || path;
  $("tree").replaceChildren(
    treeButton(lastHost?.hostname || "local", 0, null, null),
    el("li", { class: "section" }, "Workspaces"),
    ...[...counts].sort(([a], [b]) => a.localeCompare(b)).map(([path, count]) => {
      const item = treeButton(shortName(path), 1, count, path);
      item.querySelector(".label").title = path;
      return item;
    }),
    el("li", { class: "section" }, "Containers"),
    ...(lastHost?.containers || []).map((c) => el("li", { class: "guest" },
      el("button", { type: "button", "aria-current": String(selected === c.id), onclick: () => selectContainer(c.id) },
        el("span", { class: c.state, "aria-label": c.state }, CONTAINER_ICON[c.state] || "•"),
        el("span", { class: "label", title: c.name }, c.name)))),
  );
}

const CONTAINER_ICON = { running: "●", paused: "◌", restarting: "◌", created: "○", exited: "○", dead: "✗" };

function renderContainers() {
  if (!lastHost?.containers) {
    $("containers").replaceChildren(el("tr", {}, el("td", { colspan: "4", class: "meta" }, "Docker unavailable.")));
    return;
  }
  const rows = lastHost.containers.map((c) => el("tr", {
    class: "container-row", tabindex: "0", "aria-selected": String(selected === c.id),
    onclick: () => selectContainer(c.id),
    onkeydown: (event) => { if (event.key === "Enter") selectContainer(c.id); },
  },
    el("td", { title: c.id }, c.name),
    el("td", {}, c.kind || "–"),
    el("td", { title: c.image }, c.image),
    el("td", { class: c.state }, `${CONTAINER_ICON[c.state] || "•"} ${c.status}`)));
  $("containers").replaceChildren(...(rows.length ? rows : [el("tr", {}, el("td", { colspan: "4", class: "meta" }, "No Bosn containers."))]));
}

function selectContainer(id) {
  selected = id;
  renderTree();
  renderContainers();
  renderDetail();
}

// Asks Docker only while a container is selected: the sampler never pays for this.
let detailShown = null; // whose facts the panel holds
let detailCall = 0; // only the newest request may draw

async function renderDetail() {
  const id = selected;
  const container = lastHost?.containers?.find((c) => c.id === id);
  $("detail").hidden = !container;
  if (!container) {
    detailShown = null;
    return;
  }
  $("detail-title").textContent = container.name;
  const facts = (detail) => [
    ["Status", container.status], ["Image", container.image], ["Stack", container.kind || "–"], ["ID", container.id],
    ...(detail?.stats ? [["CPU", detail.stats.cpu], ["Memory", `${detail.stats.memory} (${detail.stats.memory_percent})`], ["Processes", detail.stats.pids]] : []),
  ].flatMap(([term, value]) => [el("dt", {}, term), el("dd", {}, value)]);
  // Docker can take seconds: until it answers, show only what the listing knows.
  if (detailShown !== id) {
    detailShown = id;
    $("detail-facts").replaceChildren(...facts(null));
    $("detail-labels").replaceChildren();
  }
  const call = ++detailCall;
  try {
    const detail = await request(`/v1/system/containers/${id}`);
    if (call !== detailCall) return;
    $("detail-facts").replaceChildren(...facts(detail));
    $("detail-labels").replaceChildren(...detail.labels.map((label) => el("li", {}, label)));
  } catch (error) {
    if (call !== detailCall) return;
    $("detail-facts").replaceChildren(...facts(null), el("dt", {}, "Details"), el("dd", {}, error.message));
    $("detail-labels").replaceChildren();
  }
}

function renderTasks() {
  const runs = lastRuns.filter((run) => workspace === null || run.workspace === workspace);
  const rows = runs.map((run) => {
    const word = runWord(run);
    const failed = ["failure", "error", "timed_out"].includes(word);
    return el("tr", {
      class: `task-row${failed ? " failed" : ""}`, tabindex: "0",
      onclick: () => { location.href = `/ci/runs/${run.id}`; },
      onkeydown: (event) => { if (event.key === "Enter") location.href = `/ci/runs/${run.id}`; },
    },
      el("td", {}, stamp(run.started_at ?? run.created_at)),
      el("td", {}, stamp(run.finished_at)),
      el("td", { title: run.workspace }, run.workspace),
      el("td", { title: run.workflow }, run.workflow.replace(".github/workflows/", "")),
      el("td", {}, run.actor),
      el("td", { class: word }, `${ICON[word] || "•"} ${word.replace("_", " ")}`));
  });
  $("tasks").replaceChildren(...(rows.length ? rows : [el("tr", {}, el("td", { colspan: "6", class: "meta" }, "No CI runs."))]));
}

async function refresh() {
  try {
    const [system, list, capacity] = await Promise.all([
      request("/v1/system"),
      request("/v1/runs?limit=100"),
      // A failure here is one line of the page, not the whole refresh.
      request("/v1/runners").catch((error) => ({ engine_capacity_error: error.message })),
    ]);
    lastCapacity = capacity;
    lastHost = system.host;
    lastRuns = list.runs;
    lastRunners = list.runners;
    renderStatus(system.host, system.samples[system.samples.length - 1]);
    renderCharts(system.host, system.samples);
    renderTree();
    renderTasks();
    renderContainers();
    renderDetail();
    $("live").textContent = `Updated ${new Date().toLocaleTimeString()} · last hour, ${system.interval_secs} s samples`;
  } catch (error) {
    $("live").textContent = error instanceof Unauthorized
      ? "Session expired: run `bosn ui --path /system` for a new link."
      : `Update failed: ${error.message}`;
  }
}

$("detail-close").onclick = () => selectContainer(null);
refresh();
setInterval(refresh, REFRESH_MS);
