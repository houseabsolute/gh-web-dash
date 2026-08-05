"use strict";

// Maps the server's Health values to CSS classes.
const HEALTH_CLASS = {
  success: "ok",
  failure: "bad",
  running: "run",
  neutral: "neutral",
};

function relTime(iso) {
  const secs = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (secs < 60) return Math.floor(secs) + "s ago";
  if (secs < 3600) return Math.floor(secs / 60) + "m ago";
  if (secs < 86400) return Math.floor(secs / 3600) + "h ago";
  return Math.floor(secs / 86400) + "d ago";
}

// The server sends health for summaries but raw status/conclusion for history
// runs, so the strip classifies them here with the same rules.
function runHealth(run) {
  if (run.status !== "completed") return "running";
  if (run.conclusion === "success") return "success";
  if (["failure", "timed_out", "startup_failure"].includes(run.conclusion)) return "failure";
  return "neutral";
}

function dot(health) {
  const s = document.createElement("span");
  s.className = "dot " + (HEALTH_CLASS[health] || "neutral");
  return s;
}

function cell(text, cls) {
  const td = document.createElement("td");
  if (cls) td.className = cls;
  if (text !== null) td.textContent = text;
  return td;
}

function renderRepos(repos) {
  const tbody = document.getElementById("rows");
  tbody.innerHTML = "";
  for (const repo of repos) {
    tbody.appendChild(repoRow(repo));
    if (expanded.has(repo.full_name)) tbody.appendChild(expansionRow(repo));
  }
  document.getElementById("empty").classList.toggle("hidden", repos.length > 0);
}

// A "view" pill linking out to github.com. Clicks must not also toggle the row.
function viewPill(href, label) {
  const a = document.createElement("a");
  a.className = "view";
  a.href = href;
  a.target = "_blank";
  a.rel = "noopener";
  a.textContent = "view";
  a.title = label;
  a.addEventListener("click", (e) => e.stopPropagation());
  return a;
}

// GitHub addresses a workflow page by path, not by numeric ID. The whole path
// is encoded rather than just its basename: GitHub-generated workflows have
// paths like "dynamic/github-code-scanning/codeql", where the basename alone
// resolves to nothing.
function workflowUrl(repoFullName, workflowPath) {
  return (
    "https://github.com/" +
    repoFullName +
    "/actions/workflows/" +
    encodeURIComponent(workflowPath)
  );
}

function repoRow(repo) {
  const tr = document.createElement("tr");
  tr.className = "repo-row";
  tr.addEventListener("click", () => toggleRepo(repo.full_name));

  tr.appendChild(cell(expanded.has(repo.full_name) ? "▾" : "▸", "caret"));

  const st = cell(null, "status-cell");
  st.appendChild(dot(repo.health));
  tr.appendChild(st);

  const nameCell = cell(repo.full_name, "repo");
  nameCell.appendChild(
    viewPill("https://github.com/" + repo.full_name, "Open " + repo.full_name + " on GitHub")
  );
  tr.appendChild(nameCell);
  tr.appendChild(workflowCell(repo.full_name, repo.workflows));
  tr.appendChild(cell(relTime(repo.started_at), "time"));
  return tr;
}

// Name what is broken; count what is not.
function workflowCell(repoFullName, workflows) {
  const td = cell(null, null);
  let okCount = 0;
  for (const wf of workflows) {
    if (wf.health === "success") {
      okCount++;
      continue;
    }
    const pill = document.createElement("span");
    pill.className = "wf";
    pill.appendChild(dot(wf.health));
    pill.appendChild(document.createTextNode(wf.workflow_name));
    td.appendChild(pill);
    // Only linkable once the run has been refetched with its workflow path.
    if (wf.workflow_path) {
      td.appendChild(
        viewPill(workflowUrl(repoFullName, wf.workflow_path), "Open " + wf.workflow_name + " on GitHub")
      );
    }
  }
  const note = document.createElement("span");
  note.className = "muted mono";
  if (okCount === workflows.length) {
    note.textContent = okCount === 1 ? "1 workflow" : okCount + " workflows";
  } else if (okCount > 0) {
    note.textContent = "+" + okCount + " ok";
  }
  td.appendChild(note);
  return td;
}

function expansionRow(repo) {
  const tr = document.createElement("tr");
  const td = cell(null, "exp");
  td.colSpan = 5;

  const workflows = history.get(repo.full_name);
  if (!workflows) {
    const loading = document.createElement("div");
    loading.className = "loading";
    loading.textContent = "Loading history…";
    td.appendChild(loading);
  } else if (workflows.length === 0) {
    const none = document.createElement("div");
    none.className = "loading";
    none.textContent = "No runs recorded.";
    td.appendChild(none);
  } else {
    for (const wf of workflows) td.appendChild(workflowBlock(wf));
  }

  tr.appendChild(td);
  return tr;
}

function workflowBlock(wf) {
  const div = document.createElement("div");
  div.className = "wfblock";

  // The name itself is the link here — there is room for it, unlike the
  // collapsed row where a bare name would not read as clickable.
  const latestRun = wf.runs[0];
  const linkable = latestRun && latestRun.workflow_path;
  const name = document.createElement(linkable ? "a" : "span");
  name.className = "wfname";
  name.textContent = wf.workflow_name;
  if (linkable) {
    name.href = workflowUrl(latestRun.repo_full_name, latestRun.workflow_path);
    name.target = "_blank";
    name.rel = "noopener";
    name.title = "Open " + wf.workflow_name + " on GitHub";
    name.addEventListener("click", (e) => e.stopPropagation());
  }
  div.appendChild(name);

  // Newest on the left, matching the repo's leading status dot.
  const strip = document.createElement("span");
  strip.className = "strip";
  for (const run of wf.runs) {
    strip.appendChild(runLink(run, "seg " + HEALTH_CLASS[runHealth(run)], ""));
  }
  div.appendChild(strip);

  const latest = wf.runs[0];
  if (latest) {
    const text =
      latest.branch + " · " + relTime(latest.started_at) + " · " + latest.commit_subject;
    div.appendChild(runLink(latest, "sub mono", text));
  }
  return div;
}

// A link to a run on github.com. Clicks must not also toggle the row.
function runLink(run, cls, text) {
  const a = document.createElement("a");
  a.className = cls;
  a.href = run.html_url;
  a.target = "_blank";
  a.rel = "noopener";
  a.title = run.branch + " · " + relTime(run.started_at);
  if (text) a.textContent = text;
  a.addEventListener("click", (e) => e.stopPropagation());
  return a;
}
