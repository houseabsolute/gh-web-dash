/// The fields of a workflow run that decide whether it belongs in the feed.
#[derive(Debug, Clone)]
pub struct RunCandidate {
    pub branch: String,
    pub actor_login: String,
    pub actor_type: String,
}

pub fn is_bot(actor_login: &str, actor_type: &str) -> bool {
    actor_type.eq_ignore_ascii_case("bot") || actor_login.to_ascii_lowercase().ends_with("[bot]")
}

/// Keep runs on the default branch, plus runs authored by the current user.
/// Bot-authored runs are never kept.
pub fn should_keep(c: &RunCandidate, default_branch: &str, current_user: &str) -> bool {
    if is_bot(&c.actor_login, &c.actor_type) {
        return false;
    }
    c.branch == default_branch || c.actor_login.eq_ignore_ascii_case(current_user)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(branch: &str, login: &str, actor_type: &str) -> RunCandidate {
        RunCandidate {
            branch: branch.to_string(),
            actor_login: login.to_string(),
            actor_type: actor_type.to_string(),
        }
    }

    #[test]
    fn keeps_default_branch_run_by_user() {
        assert!(should_keep(
            &cand("main", "autarch", "User"),
            "main",
            "autarch"
        ));
    }

    #[test]
    fn keeps_default_branch_run_by_another_human() {
        assert!(should_keep(
            &cand("main", "someone", "User"),
            "main",
            "autarch"
        ));
    }

    #[test]
    fn keeps_own_branch_run() {
        assert!(should_keep(
            &cand("fix-sort", "autarch", "User"),
            "main",
            "autarch"
        ));
    }

    #[test]
    fn drops_other_humans_branch_run() {
        assert!(!should_keep(
            &cand("their-fix", "someone", "User"),
            "main",
            "autarch"
        ));
    }

    #[test]
    fn drops_bot_run_on_a_branch() {
        assert!(!should_keep(
            &cand("dependabot/cargo/serde-1.0.2", "dependabot[bot]", "Bot"),
            "main",
            "autarch"
        ));
    }

    #[test]
    fn drops_bot_run_on_the_default_branch() {
        assert!(!should_keep(
            &cand("main", "dependabot[bot]", "Bot"),
            "main",
            "autarch"
        ));
    }

    #[test]
    fn drops_bot_by_login_suffix_when_type_is_wrong() {
        // Some payloads report type "User" for apps; the login suffix is the backstop.
        assert!(!should_keep(
            &cand("main", "renovate[bot]", "User"),
            "main",
            "autarch"
        ));
    }

    #[test]
    fn actor_type_check_is_case_insensitive() {
        assert!(!should_keep(
            &cand("main", "some-app", "bot"),
            "main",
            "autarch"
        ));
    }

    #[test]
    fn user_comparison_is_case_insensitive() {
        assert!(should_keep(
            &cand("fix", "AUTARCH", "User"),
            "main",
            "autarch"
        ));
    }

    #[test]
    fn respects_non_main_default_branch() {
        assert!(should_keep(
            &cand("master", "someone", "User"),
            "master",
            "autarch"
        ));
        assert!(!should_keep(
            &cand("main", "someone", "User"),
            "master",
            "autarch"
        ));
    }
}
