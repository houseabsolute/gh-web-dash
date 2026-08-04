use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;

pub const DEFAULT_CONFIG: &str = r#"# gh-web-dash configuration

# How often to poll GitHub for new workflow runs, in seconds.
poll_interval_secs = 180

# Include repositories from organizations you belong to, not just your own.
include_orgs = true

# Repositories to skip, as globs matched against "owner/repo".
ignore = []
"#;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    #[serde(default = "default_include_orgs")]
    pub include_orgs: bool,
    #[serde(default)]
    pub ignore: Vec<String>,
}

fn default_poll_interval() -> u64 {
    180
}

fn default_include_orgs() -> bool {
    true
}

impl Config {
    pub fn from_toml(s: &str) -> Result<Config> {
        let c: Config = toml::from_str(s).context("failed to parse config file")?;
        if c.poll_interval_secs == 0 {
            bail!("poll_interval_secs must be greater than 0");
        }
        Ok(c)
    }

    /// Load the config, creating it with commented defaults if absent.
    pub fn load_or_create(path: &Path) -> Result<Config> {
        if !path.exists() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("cannot create config directory {}", dir.display()))?;
            }
            std::fs::write(path, DEFAULT_CONFIG)
                .with_context(|| format!("cannot write config file {}", path.display()))?;
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read config file {}", path.display()))?;
        Config::from_toml(&text)
    }

    pub fn ignore_matcher(&self) -> Result<IgnoreMatcher> {
        let mut b = GlobSetBuilder::new();
        for pat in &self.ignore {
            let glob = Glob::new(pat).with_context(|| format!("invalid ignore glob: {pat}"))?;
            b.add(glob);
        }
        Ok(IgnoreMatcher {
            set: b.build().context("failed to build ignore matcher")?,
        })
    }
}

pub struct IgnoreMatcher {
    set: GlobSet,
}

impl IgnoreMatcher {
    pub fn is_ignored(&self, full_name: &str) -> bool {
        self.set.is_match(full_name)
    }
}

/// `~/.config/gh-web-dash/config.toml`
pub fn default_config_path() -> Result<PathBuf> {
    let dir = dirs::config_dir().context("cannot determine your config directory")?;
    Ok(dir.join("gh-web-dash").join("config.toml"))
}

/// `~/.config/gh-web-dash/runs.db`
pub fn default_db_path() -> Result<PathBuf> {
    let dir = dirs::config_dir().context("cannot determine your config directory")?;
    Ok(dir.join("gh-web-dash").join("runs.db"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_apply_to_empty_config() {
        let c = Config::from_toml("").unwrap();
        assert_eq!(c.poll_interval_secs, 180);
        assert!(c.include_orgs);
        assert!(c.ignore.is_empty());
    }

    #[test]
    fn parses_all_fields() {
        let c = Config::from_toml(
            r#"
poll_interval_secs = 60
include_orgs = false
ignore = ["autarch/old-*"]
"#,
        )
        .unwrap();
        assert_eq!(c.poll_interval_secs, 60);
        assert!(!c.include_orgs);
        assert_eq!(c.ignore, vec!["autarch/old-*".to_string()]);
    }

    #[test]
    fn rejects_zero_poll_interval() {
        let err = Config::from_toml("poll_interval_secs = 0").unwrap_err();
        assert!(err.to_string().contains("poll_interval_secs"), "got: {err}");
    }

    #[test]
    fn ignore_matcher_matches_globs_and_exact_names() {
        let c = Config::from_toml(r#"ignore = ["autarch/old-*", "autarch/scratch"]"#).unwrap();
        let m = c.ignore_matcher().unwrap();
        assert!(m.is_ignored("autarch/old-thing"));
        assert!(m.is_ignored("autarch/scratch"));
        assert!(!m.is_ignored("autarch/precious"));
        // A glob must not match across the slash separator.
        assert!(!m.is_ignored("other/old-thing"));
    }

    #[test]
    fn empty_ignore_list_matches_nothing() {
        let c = Config::from_toml("").unwrap();
        let m = c.ignore_matcher().unwrap();
        assert!(!m.is_ignored("autarch/anything"));
    }
}
