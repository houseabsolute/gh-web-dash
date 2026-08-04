use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

const SCHEMA: &str = r#"
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
"#;

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
}

#[derive(Debug, Clone)]
pub struct StoredRepo {
    pub full_name: String,
    pub default_branch: String,
}

#[derive(Debug, Clone)]
pub struct RunQuery {
    pub failures_only: bool,
    pub workflow: Option<String>,
    pub repo: Option<String>,
    pub limit: usize,
}

impl Default for RunQuery {
    fn default() -> Self {
        RunQuery {
            failures_only: false,
            workflow: None,
            repo: None,
            limit: 200,
        }
    }
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
        Ok(Store {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// A panic elsewhere must not disable the whole dashboard: a poisoned lock
    /// is recovered rather than propagated, since the connection itself is
    /// still usable.
    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
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

    pub fn set_repo_ignored(&self, full_name: &str, ignored: bool) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "UPDATE repos SET ignored = ?2 WHERE full_name = ?1",
            params![full_name, ignored as i64],
        )?;
        Ok(())
    }

    /// The stored ETag, or `None` if the repository has none — or is unknown.
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
                               conclusion, commit_sha, commit_subject, html_url, started_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET
                 status = excluded.status,
                 conclusion = excluded.conclusion,
                 commit_subject = excluded.commit_subject,
                 started_at = excluded.started_at",
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
            ],
        )?;
        Ok(())
    }

    pub fn recent_runs(&self, q: &RunQuery) -> Result<Vec<StoredRun>> {
        let mut sql = String::from(
            "SELECT runs.id, runs.repo_full_name, runs.workflow_name, runs.branch, runs.actor,
                    runs.status, runs.conclusion, runs.commit_sha, runs.commit_subject,
                    runs.html_url, runs.started_at
             FROM runs
             JOIN repos ON repos.full_name = runs.repo_full_name
             WHERE repos.ignored = 0",
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if q.failures_only {
            sql.push_str(" AND conclusion IN ('failure', 'timed_out', 'startup_failure')");
        }
        if let Some(w) = &q.workflow {
            args.push(Box::new(w.clone()));
            sql.push_str(&format!(" AND workflow_name = ?{}", args.len()));
        }
        if let Some(r) = &q.repo {
            args.push(Box::new(r.clone()));
            sql.push_str(&format!(" AND repo_full_name = ?{}", args.len()));
        }
        args.push(Box::new(q.limit as i64));
        sql.push_str(&format!(
            " ORDER BY started_at DESC, id DESC LIMIT ?{}",
            args.len()
        ));

        let conn = self.conn();
        let mut stmt = conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let rows = stmt
            .query_map(refs.as_slice(), |row| {
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
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The distinct workflow names visible under `q`, ignoring `q.workflow`
    /// itself — used to populate workflow chips so the chip set reflects the
    /// other active filters without collapsing to a single chip once one is
    /// selected.
    pub fn workflow_names_for_query(&self, q: &RunQuery) -> Result<Vec<String>> {
        let mut sql = String::from(
            "SELECT DISTINCT runs.workflow_name
             FROM runs
             JOIN repos ON repos.full_name = runs.repo_full_name
             WHERE repos.ignored = 0",
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if q.failures_only {
            sql.push_str(" AND conclusion IN ('failure', 'timed_out', 'startup_failure')");
        }
        if let Some(r) = &q.repo {
            args.push(Box::new(r.clone()));
            sql.push_str(&format!(" AND repo_full_name = ?{}", args.len()));
        }
        sql.push_str(" ORDER BY runs.workflow_name");

        let conn = self.conn();
        let mut stmt = conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let rows = stmt
            .query_map(refs.as_slice(), |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
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
