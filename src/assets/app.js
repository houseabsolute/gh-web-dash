"use strict";

const POLL_MS = 15000;
const DEFAULT_STALE_AFTER_MS = 3 * 180 * 1000; // three default poll intervals

let failuresOnly = false;
/// Repositories whose rows are open. Survives re-renders — a row that collapsed
/// on every poll would make the expansion useless.
const expanded = new Set();
/// full_name -> [{workflow_name, runs}]
const history = new Map();
let lastRepos = [];
/// When "Refresh now" was last pressed, so the header can show that the
/// resulting cycle is still running. Null when no manual sync is outstanding.
let syncRequestedAt = null;

function updateChipState() {
  for (const b of document.querySelectorAll(".chip")) {
    const on = b.dataset.key === "failures" ? failuresOnly : !failuresOnly;
    b.classList.toggle("on", on);
  }
}

for (const b of document.querySelectorAll(".chip")) {
  b.addEventListener("click", () => {
    failuresOnly = b.dataset.key === "failures" ? !failuresOnly : false;
    updateChipState();
    load();
  });
}

async function toggleRepo(fullName) {
  if (expanded.has(fullName)) {
    expanded.delete(fullName);
    renderRepos(lastRepos);
    return;
  }
  expanded.add(fullName);
  renderRepos(lastRepos); // open immediately; the strip fills in when it arrives
  await loadHistory(fullName);
  renderRepos(lastRepos);
}

async function loadHistory(fullName) {
  try {
    const resp = await fetch("/api/history?repo=" + encodeURIComponent(fullName));
    if (!resp.ok) throw new Error("HTTP " + resp.status);
    const data = await resp.json();
    history.set(fullName, data.workflows);
  } catch (e) {
    console.warn("history fetch failed", fullName, e);
  }
}

async function load() {
  const params = new URLSearchParams();
  if (failuresOnly) params.set("failures_only", "true");
  try {
    const resp = await fetch("/api/repos?" + params.toString());
    if (!resp.ok) throw new Error("HTTP " + resp.status);
    const data = await resp.json();
    lastRepos = data.repos;
    // Keep open rows live without fetching history for the other ~99.
    await Promise.all([...expanded].map(loadHistory));
    renderRepos(lastRepos);
  } catch (e) {
    // Leave the last-good table on screen; the header shows staleness.
    console.warn("repos fetch failed", e);
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
    // A cycle over ~100 repositories takes minutes, so a manual sync shows
    // nothing for a long time unless we say it is running. Hold "syncing…"
    // until a cycle finishes that started after the button was pressed.
    const syncPending =
      syncRequestedAt !== null &&
      (!s.last_success || new Date(s.last_success).getTime() < syncRequestedAt);
    if (!syncPending) syncRequestedAt = null;

    if (syncPending || !s.last_success) {
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
      errs.textContent =
        s.error_count + (s.error_count === 1 ? " repo failing" : " repos failing");
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
  syncRequestedAt = Date.now();
  document.getElementById("staleness").textContent = "syncing…";
  await fetch("/api/sync", { method: "POST" });
});

load();
setInterval(load, POLL_MS);
