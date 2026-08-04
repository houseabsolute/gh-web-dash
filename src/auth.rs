use anyhow::{bail, Result};

/// Decide which token to use. Pure — takes what the world reported.
pub fn choose_token(gh_output: Option<String>, env_token: Option<String>) -> Result<String> {
    for s in [gh_output, env_token].into_iter().flatten() {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    bail!(
        "No GitHub token found. Either run `gh auth login`, or set GITHUB_TOKEN \
         in your environment."
    )
}

/// Resolve a token from the real world: `gh auth token`, then `$GITHUB_TOKEN`.
pub async fn resolve_token() -> Result<String> {
    let gh_output = match tokio::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .await
    {
        Ok(out) if out.status.success() => Some(String::from_utf8_lossy(&out.stdout).into_owned()),
        // `gh` missing or not logged in — not an error yet, the env var may work.
        _ => None,
    };
    choose_token(gh_output, std::env::var("GITHUB_TOKEN").ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_gh_token() {
        let t = choose_token(Some("gh-token".into()), Some("env-token".into())).unwrap();
        assert_eq!(t, "gh-token");
    }

    #[test]
    fn falls_back_to_env() {
        let t = choose_token(None, Some("env-token".into())).unwrap();
        assert_eq!(t, "env-token");
    }

    #[test]
    fn blank_gh_output_is_not_a_token() {
        let t = choose_token(Some("  \n".into()), Some("env-token".into())).unwrap();
        assert_eq!(t, "env-token");
    }

    #[test]
    fn trims_whitespace() {
        let t = choose_token(Some("  gh-token\n".into()), None).unwrap();
        assert_eq!(t, "gh-token");
    }

    #[test]
    fn error_mentions_both_options() {
        let err = choose_token(None, None).unwrap_err().to_string();
        assert!(err.contains("gh auth login"), "got: {err}");
        assert!(err.contains("GITHUB_TOKEN"), "got: {err}");
    }
}
