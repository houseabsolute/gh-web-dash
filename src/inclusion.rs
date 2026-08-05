//! Deciding which repositories are worth polling.
//!
//! With ~900 repositories visible and a handful producing runs, polling
//! everything costs minutes per cycle for nothing. This module is the single
//! place that decides what to skip, kept pure so the ordering — which is the
//! whole design — can be tested exhaustively.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// A repository untouched for this long is assumed dormant, unless it has runs.
pub const STALE_DAYS: i64 = 90;

/// A manual decision recorded from the web UI, overriding the automatic rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Override {
    Include,
    Exclude,
}

impl Override {
    pub fn parse(s: &str) -> Option<Override> {
        match s {
            "include" => Some(Override::Include),
            "exclude" => Some(Override::Exclude),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Override::Include => "include",
            Override::Exclude => "exclude",
        }
    }
}

/// Why a repository is not being polled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkipReason {
    /// Muted by hand in the web UI.
    Muted,
    /// Matched an `ignore` glob in the config file.
    Glob,
    /// Archived on GitHub, so it cannot run workflows at all.
    Archived,
    /// No pushes in `STALE_DAYS`, and no runs on record.
    Stale,
    /// Discovery no longer returns it: deleted, renamed, or access lost.
    Gone,
}

impl SkipReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            SkipReason::Muted => "muted",
            SkipReason::Glob => "glob",
            SkipReason::Archived => "archived",
            SkipReason::Stale => "stale",
            SkipReason::Gone => "gone",
        }
    }

    pub fn parse(s: &str) -> Option<SkipReason> {
        match s {
            "muted" => Some(SkipReason::Muted),
            "glob" => Some(SkipReason::Glob),
            "archived" => Some(SkipReason::Archived),
            "stale" => Some(SkipReason::Stale),
            "gone" => Some(SkipReason::Gone),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Included,
    Skipped(SkipReason),
}

impl Decision {
    pub fn is_included(&self) -> bool {
        matches!(self, Decision::Included)
    }

    pub fn skip_reason(&self) -> Option<SkipReason> {
        match self {
            Decision::Included => None,
            Decision::Skipped(r) => Some(*r),
        }
    }
}

/// Everything the decision depends on, gathered by the caller.
#[derive(Debug, Clone)]
pub struct RepoFacts<'a> {
    pub user_override: Option<Override>,
    /// Matches an `ignore` glob from the config file.
    pub glob_ignored: bool,
    pub archived: bool,
    /// RFC 3339, as GitHub reports it. `None` if never seen.
    pub pushed_at: Option<&'a str>,
    /// Whether any run for this repository is stored, which keeps a
    /// scheduled-only workflow visible even with no pushes.
    pub has_runs: bool,
}

/// Decide whether to poll a repository.
///
/// Order is the design: a manual override outranks every automatic rule, so a
/// repository the rules would skip can still be rescued — otherwise a
/// scheduled-only workflow in a dormant repository would be unreachable, since
/// `ignore` globs can only subtract.
pub fn decide(f: &RepoFacts, now: DateTime<Utc>) -> Decision {
    match f.user_override {
        Some(Override::Exclude) => return Decision::Skipped(SkipReason::Muted),
        Some(Override::Include) => return Decision::Included,
        None => {}
    }
    if f.glob_ignored {
        return Decision::Skipped(SkipReason::Glob);
    }
    if f.archived {
        return Decision::Skipped(SkipReason::Archived);
    }
    if !f.has_runs && is_stale(f.pushed_at, now) {
        return Decision::Skipped(SkipReason::Stale);
    }
    Decision::Included
}

/// Unparsable or missing timestamps count as fresh: skipping a repository we
/// know nothing about would hide it with no way for the user to notice.
fn is_stale(pushed_at: Option<&str>, now: DateTime<Utc>) -> bool {
    let Some(raw) = pushed_at else { return false };
    let Ok(pushed) = DateTime::parse_from_rfc3339(raw) else {
        return false;
    };
    pushed.with_timezone(&Utc) < now - Duration::days(STALE_DAYS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-04T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn facts() -> RepoFacts<'static> {
        RepoFacts {
            user_override: None,
            glob_ignored: false,
            archived: false,
            pushed_at: Some("2026-08-01T00:00:00Z"),
            has_runs: false,
        }
    }

    #[test]
    fn a_fresh_unremarkable_repo_is_included() {
        assert_eq!(decide(&facts(), now()), Decision::Included);
    }

    #[test]
    fn globs_archived_and_stale_each_skip_with_their_own_reason() {
        let mut f = facts();
        f.glob_ignored = true;
        assert_eq!(decide(&f, now()), Decision::Skipped(SkipReason::Glob));

        let mut f = facts();
        f.archived = true;
        assert_eq!(decide(&f, now()), Decision::Skipped(SkipReason::Archived));

        let mut f = facts();
        f.pushed_at = Some("2020-01-01T00:00:00Z");
        assert_eq!(decide(&f, now()), Decision::Skipped(SkipReason::Stale));
    }

    #[test]
    fn muting_outranks_every_automatic_rule() {
        let mut f = facts();
        f.user_override = Some(Override::Exclude);
        assert_eq!(decide(&f, now()), Decision::Skipped(SkipReason::Muted));

        // Even one the rules would have included.
        f.glob_ignored = false;
        f.archived = false;
        assert_eq!(decide(&f, now()), Decision::Skipped(SkipReason::Muted));
    }

    #[test]
    fn forcing_include_rescues_a_repo_every_rule_would_skip() {
        let mut f = facts();
        f.user_override = Some(Override::Include);
        f.glob_ignored = true;
        f.archived = true;
        f.pushed_at = Some("2010-01-01T00:00:00Z");
        assert_eq!(decide(&f, now()), Decision::Included);
    }

    #[test]
    fn stored_runs_keep_a_dormant_repo_in() {
        // The scheduled-workflow case: no pushes for years, but it still runs.
        let mut f = facts();
        f.pushed_at = Some("2020-01-01T00:00:00Z");
        f.has_runs = true;
        assert_eq!(decide(&f, now()), Decision::Included);
    }

    #[test]
    fn archived_beats_stale_so_the_reason_is_the_useful_one() {
        let mut f = facts();
        f.archived = true;
        f.pushed_at = Some("2010-01-01T00:00:00Z");
        assert_eq!(decide(&f, now()), Decision::Skipped(SkipReason::Archived));
    }

    #[test]
    fn a_glob_match_reports_glob_even_when_also_archived() {
        let mut f = facts();
        f.glob_ignored = true;
        f.archived = true;
        assert_eq!(decide(&f, now()), Decision::Skipped(SkipReason::Glob));
    }

    #[test]
    fn the_staleness_boundary_is_ninety_days() {
        let mut f = facts();
        f.pushed_at = Some("2026-05-07T00:00:00Z"); // 89 days before now()
        assert_eq!(decide(&f, now()), Decision::Included);

        f.pushed_at = Some("2026-05-05T00:00:00Z"); // 91 days before now()
        assert_eq!(decide(&f, now()), Decision::Skipped(SkipReason::Stale));
    }

    #[test]
    fn unknown_or_unparsable_push_times_count_as_fresh() {
        let mut f = facts();
        f.pushed_at = None;
        assert_eq!(decide(&f, now()), Decision::Included);

        f.pushed_at = Some("not a date");
        assert_eq!(decide(&f, now()), Decision::Included);
    }

    #[test]
    fn override_strings_round_trip() {
        assert_eq!(Override::parse("include"), Some(Override::Include));
        assert_eq!(Override::parse("exclude"), Some(Override::Exclude));
        assert_eq!(Override::parse("nonsense"), None);
        assert_eq!(
            Override::parse(Override::Include.as_str()),
            Some(Override::Include)
        );
    }

    #[test]
    fn skip_reason_strings_round_trip() {
        for r in [
            SkipReason::Muted,
            SkipReason::Glob,
            SkipReason::Archived,
            SkipReason::Stale,
            SkipReason::Gone,
        ] {
            assert_eq!(SkipReason::parse(r.as_str()), Some(r));
        }
        assert_eq!(SkipReason::parse("nope"), None);
    }
}
