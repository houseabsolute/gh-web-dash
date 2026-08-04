"use strict";

const POLL_MS = 15000;
const DEFAULT_STALE_AFTER_MS = 3 * 180 * 1000; // three default poll intervals

let failuresOnly = false;
let workflow = null;

function dotClass(run) {
  if (run.status !== "completed") return "run";
  if (run.conclusion === "success") return "ok";
  if (["failure", "timed_out", "startup_failure"].includes(run.conclusion)) return "bad";
  return "other";
}

function relTime(iso) {
  const secs = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (secs < 60) return Math.floor(secs) + "s ago";
  if (secs < 3600) return Math.floor(secs / 60) + "m ago";
  if (secs < 86400) return Math.floor(secs / 3600) + "h ago";
  return Math.floor(secs / 86400) + "d ago";
}

function renderChips(workflows) {
  const chips = document.getElementById("chips");
  const wanted = ["__all__", "__failures__"].concat(workflows);
  // Rebuild only when the set of chips changed, so clicks are not lost mid-poll.
  const key = JSON.stringify(wanted);
  if (chips.dataset.keys === key) {
    updateChipState();
    return;
  }
  chips.dataset.keys = key;
  chips.innerHTML = "";
  chips.appendChild(makeChip("All", () => { failuresOnly = false; workflow = null; }, "all"));
  chips.appendChild(makeChip("Failures only", () => { failuresOnly = !failuresOnly; }, "failures"));
  for (const w of workflows) {
    chips.appendChild(makeChip(w, () => { workflow = workflow === w ? null : w; }, "wf:" + w));
  }
  updateChipState();
}

function makeChip(label, onClick, key) {
  const b = document.createElement("button");
  b.className = "chip";
  b.type = "button";
  b.textContent = label;
  b.dataset.key = key;
  b.addEventListener("click", () => {
    onClick();
    updateChipState();
    load();
  });
  return b;
}

function updateChipState() {
  for (const b of document.querySelectorAll(".chip")) {
    const k = b.dataset.key;
    let on = false;
    if (k === "all") on = !failuresOnly && workflow === null;
    else if (k === "failures") on = failuresOnly;
    else if (k.startsWith("wf:")) on = workflow === k.slice(3);
    b.classList.toggle("on", on);
  }
}

function renderRows(runs) {
  const tbody = document.getElementById("rows");
  tbody.innerHTML = "";
  for (const run of runs) {
    const tr = document.createElement("tr");
    const cells = [
      ['<span class="dot ' + dotClass(run) + '"></span>', ""],
      [run.repo_full_name, "repo"],
      [run.workflow_name, "mono muted"],
      [run.branch, "mono muted"],
      [relTime(run.started_at), ""],
    ];
    cells.forEach(([content, cls], i) => {
      const td = document.createElement("td");
      if (i === 4) td.className = "time";
      const a = document.createElement("a");
      a.href = run.html_url;
      a.target = "_blank";
      a.rel = "noopener";
      if (i === 0) a.innerHTML = content;
      else { a.textContent = content; a.className = cls; }
      td.appendChild(a);
      tr.appendChild(td);
    });
    tbody.appendChild(tr);
  }
  document.getElementById("empty").classList.toggle("hidden", runs.length > 0);
}

async function load() {
  const params = new URLSearchParams();
  if (failuresOnly) params.set("failures_only", "true");
  if (workflow) params.set("workflow", workflow);
  try {
    const resp = await fetch("/api/runs?" + params.toString());
    if (!resp.ok) throw new Error("HTTP " + resp.status);
    const data = await resp.json();
    renderChips(data.workflows);
    renderRows(data.runs);
  } catch (e) {
    // Leave the last-good table on screen; the header will show staleness.
    console.warn("runs fetch failed", e);
  }
  await loadStatus();
}

async function loadStatus() {
  const el = document.getElementById("staleness");
  const rl = document.getElementById("ratelimit");
  const errs = document.getElementById("errors");
  try {
    const resp = await fetch("/api/status");
    if (!resp.ok) throw new Error("HTTP " + resp.status);
    const s = await resp.json();
    if (!s.last_success) {
      el.textContent = "syncing…";
      el.classList.remove("stale");
    } else {
      const age = Date.now() - new Date(s.last_success).getTime();
      const staleAfterMs = s.poll_interval_secs
        ? 3 * s.poll_interval_secs * 1000
        : DEFAULT_STALE_AFTER_MS;
      el.textContent = "synced " + relTime(s.last_success);
      el.classList.toggle("stale", age > staleAfterMs);
    }
    // A cycle can "succeed" while every repository in it failed, so the error
    // count has to be shown alongside the sync time, not hidden behind it.
    errs.classList.toggle("hidden", !s.error_count);
    if (s.error_count) {
      errs.textContent = s.error_count + (s.error_count === 1 ? " repo failing" : " repos failing");
      errs.title = s.last_error || "";
    }
    const low = s.rate_limit_remaining !== null && s.rate_limit_remaining < 500;
    rl.classList.toggle("hidden", !low);
    if (low) rl.textContent = "rate limit low (" + s.rate_limit_remaining + ")";
  } catch (e) {
    el.textContent = "server unreachable";
    el.classList.add("stale");
  }
}

document.getElementById("refresh").addEventListener("click", async () => {
  await fetch("/api/sync", { method: "POST" });
  setTimeout(load, 1500);
});

load();
setInterval(load, POLL_MS);
