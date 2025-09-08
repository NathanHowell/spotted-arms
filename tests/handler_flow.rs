use std::sync::Arc;

use axum::http::HeaderMap;
use axum_github_webhook_extract::GithubToken;
use serde_json::Deserializer;

struct MockCompute;

struct MockGithub;

impl ephemeral_runner::compute::ComputeApi for MockCompute {
    fn compute_region_instance_templates_get(
        &self,
        _params: gcloud_sdk::google_rest_apis::compute_v1::region_instance_templates_api::ComputePeriodRegionInstanceTemplatesPeriodGetParams,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        gcloud_sdk::google_rest_apis::compute_v1::InstanceTemplate,
                        ephemeral_runner::compute::ComputeError,
                    >,
                > + Send,
        >,
    > {
        Box::pin(async {
            Err(ephemeral_runner::compute::ComputeError::Other(
                "unused".into(),
            ))
        })
    }

    fn compute_instances_insert(
        &self,
        _params: gcloud_sdk::google_rest_apis::compute_v1::instances_api::ComputePeriodInstancesPeriodInsertParams,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        gcloud_sdk::google_rest_apis::compute_v1::Operation,
                        ephemeral_runner::compute::ComputeError,
                    >,
                > + Send,
        >,
    > {
        Box::pin(async {
            Err(ephemeral_runner::compute::ComputeError::Other(
                "unused".into(),
            ))
        })
    }

    fn compute_instances_delete(
        &self,
        _params: gcloud_sdk::google_rest_apis::compute_v1::instances_api::ComputePeriodInstancesPeriodDeleteParams,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        gcloud_sdk::google_rest_apis::compute_v1::Operation,
                        ephemeral_runner::compute::ComputeError,
                    >,
                > + Send,
        >,
    > {
        Box::pin(async { Err(ephemeral_runner::compute::ComputeError::NotFound) })
    }
}

impl ephemeral_runner::github::GithubApi for MockGithub {
    fn generate_jit_config(
        &self,
        _repo_url: &reqwest::Url,
        _github_token: &str,
        _runner_name: &str,
        _labels: &[String],
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<String, ephemeral_runner::github::GithubError>>
                + Send,
        >,
    > {
        Box::pin(async {
            Err(ephemeral_runner::github::GithubError::Other(
                "unused".into(),
            ))
        })
    }
}

#[tokio::test]
async fn handle_completed_event_deletes_instance() {
    let headers = {
        let mut h = HeaderMap::new();
        h.insert("X-GitHub-Event", "workflow_job".parse().unwrap());
        h.insert("X-GitHub-Delivery", "test-delivery".parse().unwrap());
        h
    };

    // Load completed payload
    let body_str = include_str!("fixtures/completed-payload.json");
    let mut de = Deserializer::from_str(body_str);
    let body: ephemeral_runner::webhook::WorkflowJobWebhook =
        serde_path_to_error::deserialize(&mut de).unwrap();

    let state = ephemeral_runner::server::AppState {
        compute_client: Arc::new(MockCompute),
        github_client: Arc::new(MockGithub),
        project_id: Arc::new("test-project".to_string()),
        region: Arc::new("us-central1".to_string()),
        secret: GithubToken(Arc::new("secret".into())),
        token: Arc::new("token".into()),
    };

    let res = ephemeral_runner::webhook::handle_workflow_job_event(
        headers,
        axum::extract::State(state),
        axum_github_webhook_extract::GithubEvent(body),
    )
    .await;

    assert!(res.is_ok());
}
