use std::sync::{Arc, Mutex};

use axum::http::HeaderMap;
use axum_github_webhook_extract::GithubToken;

/// Records the name of every instance it is asked to delete.
#[derive(Default)]
struct MockCompute {
    deleted: Mutex<Vec<String>>,
}

impl MockCompute {
    fn deleted(&self) -> Vec<String> {
        self.deleted.lock().unwrap().clone()
    }
}

struct MockGithub;

impl spotted_arms::compute::ComputeApi for MockCompute {
    fn compute_region_instance_templates_get(
        &self,
        _params: gcloud_sdk::google_rest_apis::compute_v1::region_instance_templates_api::ComputePeriodRegionInstanceTemplatesPeriodGetParams,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        gcloud_sdk::google_rest_apis::compute_v1::InstanceTemplate,
                        spotted_arms::compute::ComputeError,
                    >,
                > + Send,
        >,
    > {
        Box::pin(async { Err(spotted_arms::compute::ComputeError::Other("unused".into())) })
    }

    fn compute_instances_insert(
        &self,
        _params: gcloud_sdk::google_rest_apis::compute_v1::instances_api::ComputePeriodInstancesPeriodInsertParams,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        gcloud_sdk::google_rest_apis::compute_v1::Operation,
                        spotted_arms::compute::ComputeError,
                    >,
                > + Send,
        >,
    > {
        Box::pin(async { Err(spotted_arms::compute::ComputeError::Other("unused".into())) })
    }

    fn compute_instances_delete(
        &self,
        params: gcloud_sdk::google_rest_apis::compute_v1::instances_api::ComputePeriodInstancesPeriodDeleteParams,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        gcloud_sdk::google_rest_apis::compute_v1::Operation,
                        spotted_arms::compute::ComputeError,
                    >,
                > + Send,
        >,
    > {
        self.deleted.lock().unwrap().push(params.instance);
        Box::pin(async { Ok(gcloud_sdk::google_rest_apis::compute_v1::Operation::default()) })
    }
}

impl spotted_arms::github::GithubApi for MockGithub {
    fn generate_jit_config(
        &self,
        _repo_url: &reqwest::Url,
        _github_token: &str,
        _runner_name: &str,
        _labels: &[String],
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<String, spotted_arms::github::GithubError>>
                + Send,
        >,
    > {
        Box::pin(async { Err(spotted_arms::github::GithubError::Other("unused".into())) })
    }
}

/// Runs the completed fixture through the real webhook handler, with the
/// given `workflow_job.runner_name`, and returns the instances that were deleted.
async fn deleted_for(runner_name: serde_json::Value) -> Vec<String> {
    let headers = {
        let mut h = HeaderMap::new();
        h.insert("X-GitHub-Event", "workflow_job".parse().unwrap());
        h.insert("X-GitHub-Delivery", "test-delivery".parse().unwrap());
        h
    };

    // The fixture is job 42 of run 123.
    let mut payload: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/completed-payload.json")).unwrap();
    payload["workflow_job"]["runner_name"] = runner_name;
    let body: spotted_arms::webhook::WorkflowJobWebhook =
        serde_path_to_error::deserialize(payload).unwrap();

    let compute = Arc::new(MockCompute::default());
    let state = spotted_arms::server::AppState {
        compute_client: compute.clone(),
        github_client: Arc::new(MockGithub),
        project_id: Arc::new("test-project".to_string()),
        region: Arc::new("us-central1".to_string()),
        secret: GithubToken(Arc::new("secret".into())),
        token: Arc::new("token".into()),
        instance_template: Arc::new("template".into()),
    };

    let res = spotted_arms::webhook::handle_workflow_job_event(
        headers,
        axum::extract::State(state),
        axum_github_webhook_extract::GithubEvent(body),
    )
    .await;

    assert!(res.is_ok());
    compute.deleted()
}

/// Job 123/42 ran on the VM that was created for a different job. That VM,
/// not `gha-123-42`, is the one to delete.
#[tokio::test]
async fn completed_event_deletes_the_runner_that_ran_the_job() {
    assert_eq!(deleted_for("gha-123-7".into()).await, ["gha-123-7"]);
}

#[tokio::test]
async fn completed_event_without_runner_deletes_nothing() {
    assert!(deleted_for(serde_json::Value::Null).await.is_empty());
}

#[tokio::test]
async fn completed_event_from_a_foreign_runner_deletes_nothing() {
    assert!(deleted_for("my-own-box".into()).await.is_empty());
}
