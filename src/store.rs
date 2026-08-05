use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

const SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS repos (
    full_name      TEXT PRIMARY KEY,
    default_branch TEXT NOT NULL,
    etag           TEXT,
    last_synced    TEXT,
    ignored        INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS runs (
    id             INTEGER PRIMARY KEY,
    repo_full_name TEXT NOT NULL REFERENCES repos(full_name) ON DELETE CASCADE,
    workflow_name  TEXT NOT NULL,
    branch         TEXT NOT NULL,
    actor          TEXT NOT NULL,
    status         TEXT NOT NULL,
    conclusion     TEXT,
    commit_sha     TEXT NOT NULL,
    commit_subject TEXT NOT NULL,
    html_url       TEXT NOT NULL,
    started_at     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS runs_started_at ON runs(started_at DESC);
";

/// A workflow run as stored and served. Timestamps are RFC 3339 strings, which
/// sort correctly as text and need no conversion on the way to JSON.
#[derive(Debug, Clone, Serialize)]
pub struct StoredRun {
    pub id: i64,
    pub repo_full_name: String,
    pub workflow_name: String,
    pub branch: String,
    pub actor: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub commit_sha: String,
    pub commit_subject: String,
    pub html_url: String,
    pub started_at: String,
    /// The workflow's file path, e.g. `.github/workflows/lint.yml`. GitHub's
    /// workflow page is addressed by file name, not by numeric ID — the ID
    /// route renders "This workflow does not exist". `None` until the run is
    /// refetched after the migration.
    pub workflow_path: Option<String>,
}

/// One row of the repository management page.
#[derive(Debug, Clone, Serialize)]
pub struct ManagedRepo {
    pub full_name: String,
    pub included: bool,
    pub skip_reason: Option<String>,
    pub user_override: Option<String>,
    pub archived: bool,
    pub pushed_at: Option<String>,
    pub run_count: usize,
}

/// The stored half of what the inclusion rules need.
#[derive(Debug, Clone)]
pub struct RepoFactsRow {
    pub user_override: Option<String>,
    pub archived: bool,
    pub pushed_at: Option<String>,
    pub has_runs: bool,
}

#[derive(Debug, Clone)]
pub struct StoredRepo {
    pub full_name: String,
    pub default_branch: String,
}

/// The rolled-up state of a run, a workflow, or a repository.
///
/// The ordering IS the roll-up rule: a repository's health is the maximum over
/// its workflows, so these must run least-bad to worst.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    Success,
    Neutral,
    Running,
    Failure,
}

impl Health {
    #[must_use]
    pub fn of(status: &str, conclusion: Option<&str>) -> Health {
        if status != "completed" {
            return Health::Running;
        }
        match conclusion {
            Some("success") => Health::Success,
            Some("failure" | "timed_out" | "startup_failure") => Health::Failure,
            // Cancelled, skipped, neutral, action_required: not broken, not green.
            _ => Health::Neutral,
        }
    }
}

/// The latest run of one workflow, as the row and its expansion display it.
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowSummary {
    pub workflow_name: String,
    /// `None` until the run is refetched after the migration; the page omits
    /// the link rather than pointing at a workflow that cannot be resolved.
    pub workflow_path: Option<String>,
    pub health: Health,
    pub branch: String,
    pub commit_subject: String,
    pub html_url: String,
    pub started_at: String,
}

/// One repository row.
#[derive(Debug, Clone, Serialize)]
pub struct RepoSummary {
    pub full_name: String,
    pub health: Health,
    /// The newest `started_at` across the workflows below. Drives the sort.
    pub started_at: String,
    pub workflows: Vec<WorkflowSummary>,
}

/// One workflow's recent runs, newest first — the expansion's history strip.
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowHistory {
    pub workflow_name: String,
    pub runs: Vec<StoredRun>,
}

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("cannot create data directory {}", dir.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("cannot open database {}", path.display()))?;
        Store::init(conn)
    }

    pub fn open_in_memory() -> Result<Store> {
        Store::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Store> {
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        conn.execute_batch(SCHEMA)
            .context("failed to create schema")?;
        Store::migrate(&conn).context("failed to migrate schema")?;
        Ok(Store {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Bring an older database up to date. Idempotent: each step checks whether
    /// it has already been applied, so this runs on every open and does nothing
    /// after the first time.
    fn migrate(conn: &Connection) -> Result<()> {
        let has_column = |name: &str| -> Result<bool> {
            Ok(conn
                .prepare("SELECT 1 FROM pragma_table_info('runs') WHERE name = ?1")?
                .exists([name])?)
        };

        // An earlier version stored a numeric workflow ID. GitHub addresses a
        // workflow page by file name, so the ID was useless — drop it.
        if has_column("workflow_id")? {
            conn.execute_batch("ALTER TABLE runs DROP COLUMN workflow_id;")
                .context("failed to drop the obsolete runs.workflow_id")?;
        }

        for (col, decl) in [
            ("user_override", "TEXT"),
            ("archived", "INTEGER NOT NULL DEFAULT 0"),
            ("pushed_at", "TEXT"),
            ("skip_reason", "TEXT"),
        ] {
            let present = conn
                .prepare("SELECT 1 FROM pragma_table_info('repos') WHERE name = ?1")?
                .exists([col])?;
            if !present {
                conn.execute_batch(&format!("ALTER TABLE repos ADD COLUMN {col} {decl};"))
                    .with_context(|| format!("failed to add repos.{col}"))?;
            }
        }

        if !has_column("workflow_path")? {
            // Clearing the ETags in the same step is what backfills the new
            // column: every repository looks changed on the next cycle, so its
            // runs are refetched once with the workflow ID attached. Doing it
            // here means it happens exactly once, on the first open after the
            // upgrade — a restart must not force a full resync.
            conn.execute_batch(
                "BEGIN;
                 ALTER TABLE runs ADD COLUMN workflow_path TEXT;
                 UPDATE repos SET etag = NULL;
                 COMMIT;",
            )
            .context("failed to add runs.workflow_path")?;
        }
        Ok(())
    }

    /// A panic elsewhere must not disable the whole dashboard: a poisoned lock
    /// is recovered rather than propagated, since the connection itself is
    /// still usable.
    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn upsert_repo(&self, full_name: &str, default_branch: &str) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO repos (full_name, default_branch) VALUES (?1, ?2)
             ON CONFLICT(full_name) DO UPDATE SET default_branch = excluded.default_branch",
            params![full_name, default_branch],
        )?;
        Ok(())
    }

    /// Record what GitHub says about a repository, used by the inclusion rules.
    pub fn set_repo_facts(
        &self,
        full_name: &str,
        archived: bool,
        pushed_at: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "UPDATE repos SET archived = ?2, pushed_at = ?3 WHERE full_name = ?1",
            params![full_name, i64::from(archived), pushed_at],
        )?;
        Ok(())
    }

    /// Apply an inclusion decision: the effective flag plus the reason the UI
    /// shows. Kept together so the two can never disagree.
    pub fn set_repo_decision(
        &self,
        full_name: &str,
        included: bool,
        skip_reason: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "UPDATE repos SET ignored = ?2, skip_reason = ?3 WHERE full_name = ?1",
            params![full_name, i64::from(!included), skip_reason],
        )?;
        Ok(())
    }

    pub fn set_repo_override(&self, full_name: &str, value: Option<&str>) -> Result<()> {
        let conn = self.conn();
        let n = conn.execute(
            "UPDATE repos SET user_override = ?2 WHERE full_name = ?1",
            params![full_name, value],
        )?;
        if n == 0 {
            anyhow::bail!("unknown repository: {full_name}");
        }
        Ok(())
    }

    /// Every repository with the state the management page displays.
    pub fn managed_repos(&self) -> Result<Vec<ManagedRepo>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT r.full_name, r.ignored, r.skip_reason, r.user_override, r.archived,
                    r.pushed_at, (SELECT count(*) FROM runs u WHERE u.repo_full_name = r.full_name)
             FROM repos r
             ORDER BY r.full_name",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ManagedRepo {
                    full_name: row.get(0)?,
                    included: row.get::<_, i64>(1)? == 0,
                    skip_reason: row.get(2)?,
                    user_override: row.get(3)?,
                    archived: row.get::<_, i64>(4)? != 0,
                    pushed_at: row.get(5)?,
                    // Counts from SQLite are non-negative and far below
                    // usize::MAX; a saturating conversion documents that
                    // without pretending a failure case exists.
                    run_count: usize::try_from(row.get::<_, i64>(6)?).unwrap_or(0),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The facts the inclusion rules need for one repository.
    pub fn repo_facts(&self, full_name: &str) -> Result<Option<RepoFactsRow>> {
        let conn = self.conn();
        let row = conn
            .query_row(
                "SELECT r.user_override, r.archived, r.pushed_at,
                        (SELECT count(*) FROM runs u WHERE u.repo_full_name = r.full_name)
                 FROM repos r WHERE r.full_name = ?1",
                params![full_name],
                |row| {
                    Ok(RepoFactsRow {
                        user_override: row.get(0)?,
                        archived: row.get::<_, i64>(1)? != 0,
                        pushed_at: row.get(2)?,
                        has_runs: row.get::<_, i64>(3)? > 0,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    pub fn set_repo_ignored(&self, full_name: &str, ignored: bool) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "UPDATE repos SET ignored = ?2 WHERE full_name = ?1",
            params![full_name, i64::from(ignored)],
        )?;
        Ok(())
    }

    /// The stored `ETag`, or `None` if the repository has none — or is unknown.
    pub fn repo_etag(&self, full_name: &str) -> Result<Option<String>> {
        let conn = self.conn();
        let etag = conn
            .query_row(
                "SELECT etag FROM repos WHERE full_name = ?1",
                params![full_name],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?;
        Ok(etag.flatten())
    }

    pub fn set_repo_etag(&self, full_name: &str, etag: &str) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "UPDATE repos SET etag = ?2 WHERE full_name = ?1",
            params![full_name, etag],
        )?;
        Ok(())
    }

    /// Every stored repository's full name, ignored or not.
    pub fn all_repo_names(&self) -> Result<Vec<String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT full_name FROM repos")?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn active_repos(&self) -> Result<Vec<StoredRepo>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT full_name, default_branch FROM repos WHERE ignored = 0 ORDER BY full_name",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(StoredRepo {
                    full_name: row.get(0)?,
                    default_branch: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn upsert_run(&self, r: &StoredRun) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO runs (id, repo_full_name, workflow_name, branch, actor, status,
                               conclusion, commit_sha, commit_subject, html_url, started_at, workflow_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(id) DO UPDATE SET
                 status = excluded.status,
                 conclusion = excluded.conclusion,
                 commit_subject = excluded.commit_subject,
                 started_at = excluded.started_at,
                 workflow_path = excluded.workflow_path",
            params![
                r.id,
                r.repo_full_name,
                r.workflow_name,
                r.branch,
                r.actor,
                r.status,
                r.conclusion,
                r.commit_sha,
                r.commit_subject,
                r.html_url,
                r.started_at,
                r.workflow_path,
            ],
        )?;
        Ok(())
    }

    /// One row per non-ignored repository that has runs, carrying the latest
    /// run of each of its workflows. Sorted newest activity first.
    pub fn repo_summaries(&self, failures_only: bool) -> Result<Vec<RepoSummary>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT repo_full_name, workflow_name, status, conclusion, branch,
                    commit_subject, html_url, started_at, workflow_path
             FROM (
                 SELECT runs.repo_full_name, runs.workflow_name, runs.status, runs.conclusion,
                        runs.branch, runs.commit_subject, runs.html_url, runs.started_at,
                        runs.workflow_path,
                        ROW_NUMBER() OVER (
                            PARTITION BY runs.repo_full_name, runs.workflow_name
                            ORDER BY runs.started_at DESC, runs.id DESC
                        ) AS rn
                 FROM runs
                 JOIN repos ON repos.full_name = runs.repo_full_name
                 WHERE repos.ignored = 0
             )
             WHERE rn = 1
             ORDER BY repo_full_name, workflow_name",
        )?;

        // (repo, workflow-summary) pairs, grouped below. The query already
        // orders by repo, so groups arrive contiguously.
        let rows = stmt
            .query_map([], |row| {
                let status: String = row.get(2)?;
                let conclusion: Option<String> = row.get(3)?;
                Ok((
                    row.get::<_, String>(0)?,
                    WorkflowSummary {
                        workflow_name: row.get(1)?,
                        workflow_path: row.get(8)?,
                        health: Health::of(&status, conclusion.as_deref()),
                        branch: row.get(4)?,
                        commit_subject: row.get(5)?,
                        html_url: row.get(6)?,
                        started_at: row.get(7)?,
                    },
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut summaries: Vec<RepoSummary> = Vec::new();
        for (repo, wf) in rows {
            match summaries.last_mut() {
                Some(last) if last.full_name == repo => {
                    last.health = last.health.max(wf.health);
                    if wf.started_at > last.started_at {
                        last.started_at.clone_from(&wf.started_at);
                    }
                    last.workflows.push(wf);
                }
                _ => summaries.push(RepoSummary {
                    full_name: repo,
                    health: wf.health,
                    started_at: wf.started_at.clone(),
                    workflows: vec![wf],
                }),
            }
        }

        if failures_only {
            summaries.retain(|r| r.health == Health::Failure);
        }
        // Newest activity first; name as a stable tiebreak.
        summaries.sort_by(|a, b| {
            b.started_at
                .cmp(&a.started_at)
                .then_with(|| a.full_name.cmp(&b.full_name))
        });
        Ok(summaries)
    }

    /// Recent runs for one repository, grouped by workflow, newest first within
    /// each group and capped at `per_workflow` each.
    pub fn repo_history(
        &self,
        full_name: &str,
        per_workflow: usize,
    ) -> Result<Vec<WorkflowHistory>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, repo_full_name, workflow_name, branch, actor, status, conclusion,
                    commit_sha, commit_subject, html_url, started_at, workflow_path
             FROM (
                 SELECT runs.*,
                        ROW_NUMBER() OVER (
                            PARTITION BY runs.workflow_name
                            ORDER BY runs.started_at DESC, runs.id DESC
                        ) AS rn
                 FROM runs
                 JOIN repos ON repos.full_name = runs.repo_full_name
                 WHERE repos.ignored = 0 AND runs.repo_full_name = ?1
             )
             WHERE rn <= ?2
             ORDER BY workflow_name, started_at DESC, id DESC",
        )?;

        let rows = stmt
            .query_map(
                params![full_name, i64::try_from(per_workflow).unwrap_or(i64::MAX)],
                |row| {
                    Ok(StoredRun {
                        id: row.get(0)?,
                        repo_full_name: row.get(1)?,
                        workflow_name: row.get(2)?,
                        branch: row.get(3)?,
                        actor: row.get(4)?,
                        status: row.get(5)?,
                        conclusion: row.get(6)?,
                        commit_sha: row.get(7)?,
                        commit_subject: row.get(8)?,
                        html_url: row.get(9)?,
                        started_at: row.get(10)?,
                        workflow_path: row.get(11)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut out: Vec<WorkflowHistory> = Vec::new();
        for run in rows {
            match out.last_mut() {
                Some(last) if last.workflow_name == run.workflow_name => last.runs.push(run),
                _ => out.push(WorkflowHistory {
                    workflow_name: run.workflow_name.clone(),
                    runs: vec![run],
                }),
            }
        }
        Ok(out)
    }

    /// Delete runs started before an RFC 3339 cutoff. Returns the number removed.
    pub fn prune_before(&self, cutoff_rfc3339: &str) -> Result<usize> {
        let conn = self.conn();
        let n = conn.execute(
            "DELETE FROM runs WHERE started_at < ?1",
            params![cutoff_rfc3339],
        )?;
        Ok(n)
    }
}
