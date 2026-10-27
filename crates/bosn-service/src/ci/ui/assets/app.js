// bosn ci dashboard: runs, the stage/job/step tree, step logs and runner
// management, live over /v1/events. Helpers (icons, `el`, `request`,
// `confirmButton`) come from /shared.js; writes are same-origin POSTs (the
// daemon checks Origin).
"use strict";

let selected = runFromPath();
let currentRun = null;
let detailRun = null;
let detailRequest = 0;

function runFromPath() {
  const m = location.pathname.match(/^\/ci\/runs\/([0-9a-f-]{36})$/);
  return m ? m[1] : null;
}

// Status is shown by icon and word, never by colour alone.
function badge(word) {
  const w = word || "queued";
  return el("span", { class: `status ${w}` }, `${ICON[w] || "•"} ${w.replace("_", " ")}`);
}

async function api(path, body) {
  try {
    return await request(path, body);
  } catch (error) {
    if (error instanceof Unauthorized) {
      $("detail").replaceChildren(el("p", { class: "hint" }, "Session expired: run `bosn ui` for a new link."));
    }
    throw error;
  }
}

const post = (path, body) => api(path, body || {});

async function refreshList() {
  const list = await api("/v1/runs?limit=100");
  const r = list.runners;
  $("runners").textContent = runnersText(r);
  $("limit").placeholder = r.limit;
  $("runs").replaceChildren(...list.runs.map((run) => el("li", {},
    el("button", {
      type: "button",
      "aria-current": String(run.id === selected),
      onclick: () => select(run.id),
    },
      el("span", {}, badge(runWord(run)), " ", run.workflow.replace(".github/workflows/", "")),
      el("span", { class: "meta" }, `${run.sha.slice(0, 12)}${run.dirty ? " +dirty" : ""} · ${run.trigger}/${run.mode} · ${run.actor}`),
    ))));
}

function select(run) {
  selected = run;
  connectRun(run);
  history.replaceState(null, "", `/ci/runs/${run}`);
  refreshList().catch(() => {});
  showRun().catch((e) => $("detail").replaceChildren(el("p", { class: "hint" }, e.message)));
}

async function showRun() {
  if (!selected) return;
  const requestId = ++detailRequest;
  const run = await api(`/v1/runs/${selected}`);
  if (requestId !== detailRequest || run.id !== selected) return;
  currentRun = run;
  if (run.id === selected && run.state === "done") {
    runFinished = true;
    if (runSource) runSource.close();
    if (runRetry) clearTimeout(runRetry);
    runRetry = null;
  }
  const report = run.state === "done" ? await api(`/v1/runs/${selected}/report?tail=40`) : null;
  if (requestId !== detailRequest || run.id !== selected) return;
  const parts = [
    el("h2", { "data-key": "title" }, `Run ${run.id}`),
    el("p", { "data-key": "state" }, badge(runWord(run)), " ", `exit ${run.exit_code ?? "–"} · ${run.workflow} · ${run.sha}${run.dirty ? " +dirty" : ""}`),
    el("p", { class: "meta", "data-key": "meta" }, `${run.trigger}/${run.mode} · ${run.actor} · cleanup: ${run.cleanup || "–"}`),
    el("div", { class: "controls", "data-key": "controls" },
      run.state === "done"
        ? el("button", { type: "button", onclick: () => post(`/v1/runs/${run.id}/retry`).then((r) => select(r.run)) }, "Retry")
        : runWord(run) === "cancelling"
          ? el("span", {}, "Cancelling")
          : confirmButton("Cancel", () => post(`/v1/runs/${run.id}/cancel`), showRun),
    ),
  ];
  if (run.reason) parts.push(el("p", { class: "meta", "data-key": "reason" }, run.reason));
  if (report && report.first_failure) {
    const f = report.first_failure;
    parts.push(el("div", { class: "failure-box", "data-key": "failure" },
      el("strong", {}, `First failure: ${f.job} — ${f.step || "?"} (exit ${f.exit_code ?? "?"})`),
      el("pre", {}, f.tail.join("\n"))));
  }
  parts.push(el("h3", { "data-key": "jobs-title" }, "Jobs"), tree(run));
  parts.push(logDetails("Full log", "full", {}, run));
  if (detailRun !== run.id) {
    $("detail").replaceChildren(...parts);
    detailRun = run.id;
  } else reconcile($("detail"), parts);
  refreshOpenLogs($("detail"), run);
}

function refreshOpenLogs(root, run) {
  for (const details of root.querySelectorAll("details[data-log-key][open]")) {
    if (details.parentElement.closest("details:not([open])")) continue;
    loadLog(details.querySelector("pre.log"), JSON.parse(details.dataset.filter), run).catch(() => {});
  }
}

// Keep keyed live nodes mounted while updating their status and job structure.
function reconcile(parent, fresh) {
  const keyed = new Map([...parent.children].filter((n) => n.dataset.key)
    .map((n) => [n.dataset.key, n]));
  let position = parent.firstChild;
  for (const next of fresh) {
    const old = next.dataset?.key ? keyed.get(next.dataset.key) : position;
    const node = old && old.nodeType === next.nodeType && old.nodeName === next.nodeName
      ? patchNode(old, next) : next;
    if (node !== position) parent.insertBefore(node, position);
    position = node.nextSibling;
  }
  while (position) { const next = position.nextSibling; position.remove(); position = next; }
}

function patchNode(old, next) {
  if (old.nodeType === Node.TEXT_NODE) {
    if (old.textContent !== next.textContent) old.textContent = next.textContent;
    return old;
  }
  if (old.matches("pre.log")) return old;
  if (old.nodeName === "BUTTON") return next; // handlers can change with run state
  for (const name of old.getAttributeNames()) {
    if (name !== "open" && !next.hasAttribute(name)) old.removeAttribute(name);
  }
  for (const name of next.getAttributeNames()) {
    if (name !== "open" && old.getAttribute(name) !== next.getAttribute(name))
      old.setAttribute(name, next.getAttribute(name));
  }
  reconcile(old, [...next.childNodes]);
  return old;
}

// Expanded jobs, steps, and their log nodes survive live updates.
const opened = new Set();
const logState = new WeakMap();
const seen = new Set(); // rows already shown once: a failure opens itself only the first time
let openedRun = null;

function seconds(ms) {
  return ms == null ? "" : ms < 1000 ? `${ms} ms` : `${Math.round(ms / 1000)}s`;
}

function row(status, name, ms) {
  const w = status || "queued";
  return el("summary", {},
    el("span", { class: `icon ${w}`, title: w.replace("_", " ") }, ICON[w] || "•"),
    el("span", { class: "name" }, name),
    el("span", { class: "duration" }, seconds(ms)));
}

function jobHeader(job) {
  const w = job.conclusion || job.status || "queued";
  const took = seconds(job.duration_ms);
  return el("summary", { class: "job-header" },
    el("span", { class: `icon ${w}` }, ICON[w] || "•"),
    el("span", {},
      el("span", { class: "job-name" }, (job.name || job.job_id || job.key).trim()),
      el("span", { class: "meta" }, ` ${w.replace("_", " ")}${took ? ` in ${took}` : ""}`)));
}

const FAILED = new Set(["failure", "error", "timed_out"]);

function collapsible(key, summary, body, filter, failed) {
  const details = el("details", { "data-key": key }, summary, body);
  if (filter) {
    details.dataset.logKey = key;
    details.dataset.filter = JSON.stringify(filter);
  }
  if (failed && !seen.has(key)) opened.add(key);
  seen.add(key);
  details.addEventListener("toggle", () => {
    if (details.open) {
      opened.add(key);
      if (currentRun) {
        if (filter) loadLog(body, filter, currentRun).catch(() => {});
        else refreshOpenLogs(details, currentRun);
      }
    } else opened.delete(key);
  });
  if (opened.has(key)) details.open = true;
  return details;
}

function logDetails(title, key, filter, run) {
  const pre = el("pre", { class: "log" }, "");
  return el("div", { class: "full-log", "data-key": "full-log" },
    collapsible(key, el("summary", {}, el("strong", {}, title)), pre, filter));
}

function tree(run) {
  if (openedRun !== run.id) { opened.clear(); seen.clear(); openedRun = run.id; }
  const jobs = ((run.tree && run.tree.groups) || []).flatMap((group) => group.jobs);
  if (!jobs.length) return el("p", { class: "hint" }, "No jobs yet.");
  return el("div", { class: "jobs", "data-key": "jobs" }, ...jobs.map((job) => {
    const steps = job.sections.map((s) => {
      const key = `step ${job.key} ${s.stage}:${s.id}`;
      const name = s.stage === "Pre" || s.stage === "Post" ? `${s.stage} ${s.name}` : s.name;
      const pre = el("pre", { class: "log" }, "");
      const filter = { job: job.key, section: `${s.stage}:${s.id}` };
      return collapsible(key, row(s.conclusion || s.status, name, s.duration_ms), pre,
        filter, FAILED.has(s.conclusion));
    });
    const body = el("div", { class: "steps" },
      job.reason ? el("p", { class: "meta" }, job.reason) : "", ...steps);
    return el("section", { class: "job", "data-key": `section ${job.key}` },
      collapsible(`job ${job.key}`, jobHeader(job), body, null, FAILED.has(job.conclusion)));
  }));
}

// Cursor-paged log. Retain rendered lines and ask only for records after the
// last cursor. The reader's position is unchanged unless they were following.
async function loadLog(pre, filter, run) {
  if (!pre || !pre.isConnected || selected !== run.id) return;
  let state = logState.get(pre);
  if (!state) {
    state = { since: 0, polled: false, placeholder: false, done: false, pending: false, loading: false };
    logState.set(pre, state);
  }
  if (state.done) return;
  if (state.polled && typeof run.log_records === "number" && state.since >= run.log_records) {
    if (run.state === "done") state.done = true;
    return;
  }
  if (state.loading) { state.pending = true; return; }
  state.loading = true;
  try {
    do {
      state.pending = false;
      let more;
      do {
        const before = state.since;
        const q = new URLSearchParams({ since_seq: before, limit: 1000, ...filter });
        const logs = await api(`/v1/runs/${run.id}/logs?${q}`);
        if (!pre.isConnected || selected !== run.id) return;
        const atBottom = pre.scrollHeight - pre.scrollTop - pre.clientHeight < 24;
        if (logs.records.length && state.placeholder) {
          pre.replaceChildren();
          state.placeholder = false;
        }
        const lines = document.createDocumentFragment();
        for (const record of logs.records) {
          const stream = record.stream === "stderr" ? "stderr" : record.stream === "stdout" ? "stdout" : "bosn";
          lines.append(el("span", { class: `log-line ${stream}` }, record.text.replace(/\n$/, "")));
        }
        pre.append(lines);
        if (atBottom) pre.scrollTop = pre.scrollHeight;
        state.since = logs.next_seq;
        state.polled = true;
        more = logs.more && state.since > before;
      } while (more);
      if (!pre.childNodes.length) {
        pre.append(el("span", { class: "log-line bosn" }, "No output."));
        state.placeholder = true;
      }
      state.done = run.state === "done" && !state.pending;
    } while (state.pending);
  } finally { state.loading = false; }
}

// Live feed: debounce refreshes; a resync refetches everything.
let pending = null;
function schedule() {
  if (pending) return;
  pending = setTimeout(() => {
    pending = null;
    refreshList().catch(() => {});
    if (selected) showRun().catch(() => {});
  }, 400);
}
function connect() {
  const source = new EventSource("/v1/events");
  source.onmessage = (message) => {
    const event = JSON.parse(message.data);
    if (event.type === "resync" || !selected || event.run === selected) schedule();
    else refreshList().catch(() => {});
  };
  source.onerror = () => { source.close(); setTimeout(connect, 2000); };
}

// A selected run replays saved status changes after a dropped connection.
let runSource = null;
let runCursor = 0;
let runRetry = null;
let runFinished = false;
function connectRun(run) {
  if (runSource) runSource.close();
  if (runRetry) clearTimeout(runRetry);
  runRetry = null;
  runCursor = 0;
  runFinished = false;
  openRunSource(run);
}
function openRunSource(run) {
  if (selected !== run || runFinished) return;
  const source = new EventSource(`/v1/runs/${run}/events?from_seq=${runCursor}`);
  runSource = source;
  source.addEventListener("status", (message) => {
    if (selected !== run) { source.close(); return; }
    const event = JSON.parse(message.data);
    runCursor = event.seq;
    schedule();
    if (event.state === "done") {
      runFinished = true;
      source.close();
    }
  });
  source.onerror = async () => {
    if (runSource !== source) return;
    source.close();
    if (selected !== run || runFinished) return;
    try {
      const snapshot = await api(`/v1/runs/${run}`);
      if (snapshot.state === "done") {
        runFinished = true;
        schedule();
        return;
      }
    } catch (_) { /* Retry after transient network errors. */ }
    if (runSource === source && selected === run && !runFinished) {
      runRetry = setTimeout(() => { runRetry = null; openRunSource(run); }, 2000);
    }
  };
}

$("drain").addEventListener("click", () => post("/v1/runners", { action: "drain" }).then(refreshList));
$("resume").addEventListener("click", () => post("/v1/runners", { action: "resume" }).then(refreshList));
$("set-limit").addEventListener("click", () => {
  const limit = Number($("limit").value);
  if (limit >= 1) post("/v1/runners", { action: "set_limit", limit }).then(refreshList);
});

const showCache = (reply) => { $("cache").textContent = cacheText(reply.cache); };
$("measure-cache").addEventListener("click", () => post("/v1/runners", { action: "cache_usage" }).then(showCache));
$("prune-slot").replaceChildren(confirmButton("Prune runs older than 7 days",
  () => post("/v1/runners", { action: "prune_cache", older_than_secs: WEEK_SECS }), refreshList));
$("clear-cache-slot").replaceChildren(confirmButton("Clear cache",
  () => post("/v1/runners", { action: "clear_cache" }), showCache));

refreshList().then(() => selected && showRun()).catch(() => {});
if (selected) connectRun(selected);
connect();
