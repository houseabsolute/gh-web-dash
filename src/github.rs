use anyhow::Result;
use serde::Deserialize;

pub const GITHUB_API: &str = "https://api.github.com";
const USER_AGENT: &str = "gh-web-dash";
// Fetched per repository across ALL its workflows, so a repo with three
// workflows yields roughly a third of this each — enough to fill the history
// strips. Same request count as 20; ETags keep warm cycles free.
const RUNS_PER_REPO: usize = 50;
const REPOS_PER_PAGE: usize = 100;
const REQUEST_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, thiserror::Error)]
pub enum GithubError {
    #[error("HTTP {status} from {url}")]
    Status { status: u16, url: String },
    #[error("request failed: {0}")]
    Transport(#[from] reqwest::Error),
}

impl GithubError {
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, GithubError::Status { status: 401, .. })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Repo {
    pub full_name: String,
    pub default_branch: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Actor {
    pub login: String,
    pub r#type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Commit {
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Run {
    pub id: i64,
    /// GitHub's ID for the workflow this run belongs to. Used to link to the
    /// workflow's own page.
    #[serde(default)]
    pub workflow_id: Option<i64>,
    #[serde(rename = "name", default)]
    pub workflow_name: String,
    #[serde(default)]
    pub head_branch: String,
    pub status: String,
    pub conclusion: Option<String>,
    #[serde(default)]
    pub head_sha: String,
    pub html_url: String,
    pub run_started_at: Option<String>,
    pub updated_at: String,
    pub actor: Actor,
    pub head_commit: Option<Commit>,
}

impl Run {
    /// The first line of the commit message.
    pub fn commit_subject(&self) -> String {
        self.head_commit
            .as_ref()
            .and_then(|c| c.message.lines().next())
            .unwrap_or("")
            .to_string()
    }

    /// When the run started, falling back to `updated_at` if GitHub omits it.
    pub fn started_at(&self) -> String {
        self.run_started_at
            .clone()
            .unwrap_or_else(|| self.updated_at.clone())
    }
}

#[derive(Debug, Deserialize)]
struct RunsBody {
    #[serde(default)]
    workflow_runs: Vec<Run>,
}

#[derive(Debug)]
pub struct RunsResponse {
    pub runs: Vec<Run>,
    pub etag: Option<String>,
    pub not_modified: bool,
    pub rate_limit_remaining: Option<i64>,
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

impl Client {
    pub fn new(base_url: String, token: String) -> Result<Client> {
        // Without a timeout, one hung request stalls the whole poll cycle.
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()?;
        Ok(Client {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
        })
    }

    fn get(&self, url: &str) -> reqwest::RequestBuilder {
        self.http
            .get(url)
            .bearer_auth(&self.token)
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
    }

    pub async fn current_user(&self) -> Result<String, GithubError> {
        #[derive(Deserialize)]
        struct User {
            login: String,
        }
        let url = format!("{}/user", self.base_url);
        let resp = self.get(&url).send().await?;
        if !resp.status().is_success() {
            return Err(GithubError::Status {
                status: resp.status().as_u16(),
                url,
            });
        }
        Ok(resp.json::<User>().await?.login)
    }

    /// All repositories the user can see. `include_orgs` controls whether
    /// organization repositories are included alongside their own.
    pub async fn list_repos(&self, include_orgs: bool) -> Result<Vec<Repo>, GithubError> {
        let affiliation = if include_orgs {
            "owner,organization_member"
        } else {
            "owner"
        };
        let mut all = Vec::new();
        let mut page = 1;
        loop {
            let url = format!("{}/user/repos", self.base_url);
            let resp = self
                .get(&url)
                .query(&[
                    ("per_page", REPOS_PER_PAGE.to_string()),
                    ("page", page.to_string()),
                    ("affiliation", affiliation.to_string()),
                    ("sort", "pushed".to_string()),
                ])
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(GithubError::Status {
                    status: resp.status().as_u16(),
                    url,
                });
            }
            let batch: Vec<Repo> = resp.json().await?;
            let done = batch.len() < REPOS_PER_PAGE;
            all.extend(batch);
            if done {
                return Ok(all);
            }
            page += 1;
        }
    }

    pub async fn list_runs(
        &self,
        full_name: &str,
        etag: Option<&str>,
    ) -> Result<RunsResponse, GithubError> {
        let url = format!("{}/repos/{}/actions/runs", self.base_url, full_name);
        let mut req = self
            .get(&url)
            .query(&[("per_page", RUNS_PER_REPO.to_string())]);
        if let Some(tag) = etag {
            req = req.header("if-none-match", tag);
        }
        let resp = req.send().await?;

        let rate_limit_remaining = resp
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<i64>().ok());
        let new_etag = resp
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        if resp.status().as_u16() == 304 {
            return Ok(RunsResponse {
                runs: Vec::new(),
                etag: new_etag,
                not_modified: true,
                rate_limit_remaining,
            });
        }
        if !resp.status().is_success() {
            return Err(GithubError::Status {
                status: resp.status().as_u16(),
                url,
            });
        }
        let body: RunsBody = resp.json().await?;
        Ok(RunsResponse {
            runs: body.workflow_runs,
            etag: new_etag,
            not_modified: false,
            rate_limit_remaining,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(server: &MockServer) -> Client {
        Client::new(server.uri(), "test-token".to_string()).unwrap()
    }

    #[tokio::test]
    async fn fetches_current_user() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "login": "autarch"
            })))
            .mount(&server)
            .await;

        assert_eq!(client(&server).current_user().await.unwrap(), "autarch");
    }

    #[tokio::test]
    async fn lists_repos_across_pages() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/repos"))
            .and(query_param("page", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"full_name": "autarch/a", "default_branch": "main"}
            ])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/user/repos"))
            .and(query_param("page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        let repos = client(&server).list_repos(false).await.unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].full_name, "autarch/a");
        assert_eq!(repos[0].default_branch, "main");
    }

    #[tokio::test]
    async fn fetches_runs_and_reports_etag_and_rate_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/autarch/a/actions/runs"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("etag", "W/\"abc\"")
                    .insert_header("x-ratelimit-remaining", "4321")
                    .set_body_json(serde_json::json!({
                        "workflow_runs": [{
                            "id": 42,
                            "name": "test.yml",
                            "head_branch": "main",
                            "status": "completed",
                            "conclusion": "failure",
                            "head_sha": "abc123",
                            "html_url": "https://github.com/autarch/a/actions/runs/42",
                            "run_started_at": "2026-08-04T10:00:00Z",
                            "updated_at": "2026-08-04T10:05:00Z",
                            "actor": {"login": "autarch", "type": "User"},
                            "head_commit": {"message": "Fix the thing\n\nDetails here"}
                        }]
                    })),
            )
            .mount(&server)
            .await;

        let resp = client(&server).list_runs("autarch/a", None).await.unwrap();
        assert!(!resp.not_modified);
        assert_eq!(resp.etag.as_deref(), Some("W/\"abc\""));
        assert_eq!(resp.rate_limit_remaining, Some(4321));
        assert_eq!(resp.runs.len(), 1);
        let r = &resp.runs[0];
        assert_eq!(r.id, 42);
        assert_eq!(r.workflow_name, "test.yml");
        assert_eq!(r.actor.login, "autarch");
        // Only the commit subject, not the body.
        assert_eq!(r.commit_subject(), "Fix the thing");
    }

    #[tokio::test]
    async fn not_modified_yields_no_runs() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/autarch/a/actions/runs"))
            .and(header("if-none-match", "W/\"abc\""))
            .respond_with(ResponseTemplate::new(304))
            .mount(&server)
            .await;

        let resp = client(&server)
            .list_runs("autarch/a", Some("W/\"abc\""))
            .await
            .unwrap();
        assert!(resp.not_modified);
        assert!(resp.runs.is_empty());
    }

    #[tokio::test]
    async fn missing_repo_is_an_error_not_a_panic() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/autarch/gone/actions/runs"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let err = client(&server)
            .list_runs("autarch/gone", None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, GithubError::Status { status: 404, .. }),
            "got: {err:?}"
        );
    }

    #[tokio::test]
    async fn unauthorized_is_distinguishable() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/autarch/a/actions/runs"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let err = client(&server)
            .list_runs("autarch/a", None)
            .await
            .unwrap_err();
        assert!(err.is_unauthorized(), "got: {err:?}");
    }

    #[test]
    fn commit_subject_handles_missing_commit() {
        let r = Run {
            id: 1,
            workflow_id: None,
            workflow_name: "test.yml".into(),
            head_branch: "main".into(),
            status: "completed".into(),
            conclusion: None,
            head_sha: "abc".into(),
            html_url: "https://example.com".into(),
            run_started_at: None,
            updated_at: "2026-08-04T10:00:00Z".into(),
            actor: Actor {
                login: "autarch".into(),
                r#type: "User".into(),
            },
            head_commit: None,
        };
        assert_eq!(r.commit_subject(), "");
        // With no run_started_at, fall back to updated_at so ordering still works.
        assert_eq!(r.started_at(), "2026-08-04T10:00:00Z");
    }
}
