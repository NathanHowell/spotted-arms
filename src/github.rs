use reqwest::Url;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use thiserror::Error;
use tracing::instrument;

#[derive(Debug, Error)]
pub enum GithubError {
    #[error("github api error: {0}")]
    Other(String),
}

pub trait GithubApi: Send + Sync {
    fn generate_jit_config(
        &self,
        repo_url: &Url,
        github_token: &str,
        runner_name: &str,
        labels: &[String],
    ) -> Pin<Box<dyn Future<Output = Result<String, GithubError>> + Send>>;
}

#[derive(Clone)]
pub struct GithubClient {
    client: reqwest::Client,
}

impl GithubClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

impl GithubApi for GithubClient {
    #[instrument(skip(self, github_token))]
    fn generate_jit_config(
        &self,
        repo_url: &Url,
        github_token: &str,
        runner_name: &str,
        labels: &[String],
    ) -> Pin<Box<dyn Future<Output = Result<String, GithubError>> + Send>> {
        let client = self.client.clone();
        let repo_url = repo_url.clone();
        let labels = labels.to_vec();
        let runner_name = runner_name.to_string();
        let token = github_token.to_string();

        Box::pin(async move {
            let body = serde_json::json!({
                "name": runner_name,
                "labels": labels,
                "runner_group_id": 1,
            });

            let req = client
                .post(format!("{repo_url}/actions/runners/generate-jitconfig"))
                .bearer_auth(&token)
                .header("Accept", "application/vnd.github+json")
                .header("User-Agent", "gha-autoscaler")
                .header("Content-Type", "application/json")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .json(&body)
                .build()
                .map_err(|e| GithubError::Other(e.to_string()))?;

            let resp = client
                .execute(req)
                .await
                .map_err(|e| GithubError::Other(e.to_string()))?;

            if let Err(err) = resp.error_for_status_ref() {
                let body = resp.text().await.ok();
                tracing::error!(err = %err, body = ?body, "Failed to generate JIT config");
                return Err(GithubError::Other(err.to_string()));
            }

            let json: Value = resp
                .json()
                .await
                .map_err(|e| GithubError::Other(e.to_string()))?;

            json.get("encoded_jit_config")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .ok_or_else(|| GithubError::Other("encoded_jit_config missing".to_string()))
        })
    }
}
