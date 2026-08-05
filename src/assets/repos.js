"use strict";

// Why a repository is skipped, in the words the page uses.
const REASON_LABEL = {
  muted: "muted by you",
  glob: "ignore glob",
  archived: "archived",
  stale: "no pushes in 90d",
  gone: "gone from GitHub",
};

// "included" is not a skip reason, but it is a filter category.
const CATEGORIES = ["included", "muted", "glob", "archived", "stale", "gone"];

let all = [];
let category = null;
let search = "";

function categoryOf(repo) {
  return repo.included ? "included" : repo.skip_reason || "gone";
}

function relDate(iso) {
  if (!iso) return "never";
  const days = Math.floor((Date.now() - new Date(iso).getTime()) / 86400000);
  if (days < 1) return "today";
  if (days < 60) return days + "d ago";
  return Math.floor(days / 30) + "mo ago";
}

function matches(repo) {
  if (category && categoryOf(repo) !== category) return false;
  if (search && !repo.full_name.toLowerCase().includes(search)) return false;
  return true;
}

function renderCounts() {
  const box = document.getElementById("counts");
  box.innerHTML = "";
  const counts = {};
  for (const r of all) {
    const c = categoryOf(r);
    counts[c] = (counts[c] || 0) + 1;
  }
  for (const c of CATEGORIES) {
    if (!counts[c] && c !== "included") continue;
    const b = document.createElement("button");
    b.className = "chip" + (category === c ? " on" : "");
    b.type = "button";
    b.textContent = (c === "included" ? "included" : REASON_LABEL[c]) + " " + (counts[c] || 0);
    b.addEventListener("click", () => {
      category = category === c ? null : c;
      render();
    });
    box.appendChild(b);
  }
  const clear = document.createElement("button");
  clear.className = "chip" + (category === null ? " on" : "");
  clear.type = "button";
  clear.textContent = "all " + all.length;
  clear.addEventListener("click", () => {
    category = null;
    render();
  });
  box.insertBefore(clear, box.firstChild);
}

function cell(text, cls) {
  const td = document.createElement("td");
  if (cls) td.className = cls;
  if (text !== null) td.textContent = text;
  return td;
}

function overrideControl(repo) {
  const td = cell(null, "override");
  // Three states, but only two are ever useful at once: a repo the rules
  // include needs muting, one they skip needs forcing in.
  const wanted = repo.included ? "exclude" : "include";
  const label = repo.included ? "mute" : "include anyway";

  if (repo.user_override) {
    const undo = document.createElement("button");
    undo.className = "chip on";
    undo.type = "button";
    undo.textContent = repo.user_override === "include" ? "forced in — undo" : "muted — undo";
    undo.addEventListener("click", () => setOverride(repo.full_name, null));
    td.appendChild(undo);
    return td;
  }

  const b = document.createElement("button");
  b.className = "chip";
  b.type = "button";
  b.textContent = label;
  b.addEventListener("click", () => setOverride(repo.full_name, wanted));
  td.appendChild(b);
  return td;
}

function render() {
  renderCounts();
  const tbody = document.getElementById("rows");
  tbody.innerHTML = "";
  const shown = all.filter(matches);
  for (const repo of shown) {
    const tr = document.createElement("tr");

    const st = cell(null, "status-cell");
    const dot = document.createElement("span");
    dot.className = "dot " + (repo.included ? "ok" : "neutral");
    st.appendChild(dot);
    tr.appendChild(st);

    const name = cell(repo.full_name, "repo");
    const link = document.createElement("a");
    link.className = "view";
    link.href = "https://github.com/" + repo.full_name;
    link.target = "_blank";
    link.rel = "noopener";
    link.textContent = "view";
    name.appendChild(link);
    tr.appendChild(name);

    const why = repo.included ? "included" : REASON_LABEL[repo.skip_reason] || "skipped";
    tr.appendChild(cell(why, "mono muted"));
    tr.appendChild(cell(repo.run_count ? repo.run_count + " runs" : "no runs", "mono muted"));
    tr.appendChild(cell("pushed " + relDate(repo.pushed_at), "mono muted"));
    tr.appendChild(overrideControl(repo));
    tbody.appendChild(tr);
  }
  document.getElementById("empty").classList.toggle("hidden", shown.length > 0);
  document.getElementById("summary").textContent =
    shown.length + " of " + all.length + " shown";
}

async function setOverride(repo, value) {
  try {
    const resp = await fetch("/api/override", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ repo, value }),
    });
    if (!resp.ok) throw new Error("HTTP " + resp.status);
    // The server re-evaluates and returns the whole list, so the row's reason
    // updates immediately rather than waiting for the next discovery pass.
    all = (await resp.json()).repos;
    render();
  } catch (e) {
    console.warn("override failed", repo, e);
    document.getElementById("summary").textContent = "could not update " + repo;
  }
}

async function load() {
  try {
    const resp = await fetch("/api/managed");
    if (!resp.ok) throw new Error("HTTP " + resp.status);
    all = (await resp.json()).repos;
    render();
  } catch (e) {
    document.getElementById("summary").textContent = "server unreachable";
  }
}

document.getElementById("search").addEventListener("input", (e) => {
  search = e.target.value.trim().toLowerCase();
  render();
});

load();
