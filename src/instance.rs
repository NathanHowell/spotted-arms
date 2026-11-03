//! Async interpreter for the pure state machine in `machine.rs`: runs each
//! [`Effect`] against the real GitHub and Compute APIs and feeds the results
//! back until the handler is done.

use crate::compute::{ComputeApi, ComputeError};
use crate::github::GithubApi;
use crate::machine::{Config, Effect, EffectError, Handler, JobEvent, Outcome, Response, Step};
use axum::response::ErrorResponse;
use futures_util::future::join_all;
use gcloud_sdk::google_rest_apis::compute_v1;
use gcloud_sdk::google_rest_apis::compute_v1::Instance;
use gcloud_sdk::google_rest_apis::compute_v1::instances_api::{
    ComputePeriodInstancesPeriodDeleteParams, ComputePeriodInstancesPeriodInsertParams,
};
use gcloud_sdk::google_rest_apis::compute_v1::region_instance_templates_api::ComputePeriodRegionInstanceTemplatesPeriodGetParams;
use serde_json::Value;
use tracing::{Span, field, info, instrument};

/// Everything the interpreter needs to talk to the outside world.
pub struct Deps<'a> {
    pub compute: &'a dyn ComputeApi,
    pub github: &'a dyn GithubApi,
    pub project_id: &'a str,
    pub region: &'a str,
    pub github_token: &'a str,
    pub instance_template: &'a str,
}

fn add_event_fields_to_span(event: &crate::webhook::WorkflowJobWebhook) {
    let payload = &event.payload;

    Span::current()
        .record("repo_url", field::display(&event.repository.url))
        .record("repository", event.repository.full_name.as_deref())
        .record(
            "job_id",
            payload.workflow_job.get("id").and_then(Value::as_i64),
        )
        .record(
            "run_id",
            payload.workflow_job.get("run_id").and_then(Value::as_i64),
        )
        .record(
            "run_attempt",
            payload
                .workflow_job
                .get("run_attempt")
                .and_then(Value::as_i64),
        )
        .record(
            "conclusion",
            payload
                .workflow_job
                .get("conclusion")
                .and_then(Value::as_str),
        );
}

fn effect_error(e: ComputeError) -> EffectError {
    match e {
        ComputeError::NotFound => EffectError::NotFound,
        ComputeError::Other(message) => EffectError::Other(message),
    }
}

impl Deps<'_> {
    async fn run(&self, effect: &Effect, event: &JobEvent) -> Response {
        match effect {
            Effect::GenerateJit {
                runner_name,
                labels,
            } => Response::Jit(
                self.github
                    .generate_jit_config(&event.repo_url, self.github_token, runner_name, labels)
                    .await
                    .map_err(|e| {
                        tracing::error!(?e, "Failed to generate JIT config");
                        EffectError::Other(e.to_string())
                    }),
            ),
            Effect::GetTemplateMetadata => Response::TemplateMetadata(
                self.compute
                    .compute_region_instance_templates_get(
                        ComputePeriodRegionInstanceTemplatesPeriodGetParams {
                            project: self.project_id.to_string(),
                            region: self.region.to_string(),
                            instance_template: self.instance_template.to_string(),
                            fields: Some("properties.metadata".to_string()),
                            ..Default::default()
                        },
                    )
                    .await
                    .map(|template| {
                        template
                            .properties
                            .and_then(|p| p.metadata)
                            .and_then(|m| m.items)
                            .unwrap_or_default()
                            .into_iter()
                            .filter_map(|item| Some((item.key?, item.value.unwrap_or_default())))
                            .collect()
                    })
                    .map_err(|e| {
                        tracing::error!(?e, "Failed to get instance template metadata");
                        effect_error(e)
                    }),
            ),
            Effect::InsertInstance {
                name,
                zone,
                metadata,
            } => {
                let source_instance_template = format!(
                    "projects/{}/regions/{}/instanceTemplates/{}",
                    self.project_id, self.region, self.instance_template
                );
                info!(source_instance_template, zone, "Using instance template");

                // there isn't a way to merge metadata items, so the machine
                // hands us the merged list
                let items = metadata
                    .iter()
                    .map(|(key, value)| compute_v1::MetadataItemsInner {
                        key: Some(key.clone()),
                        value: Some(value.clone()),
                    })
                    .collect();

                Response::Inserted(
                    self.compute
                        .compute_instances_insert(ComputePeriodInstancesPeriodInsertParams {
                            project: self.project_id.to_string(),
                            zone: zone.clone(),
                            source_instance_template: Some(source_instance_template),
                            instance: Some(Instance {
                                name: Some(name.clone()),
                                metadata: Some(
                                    compute_v1::Metadata {
                                        items: Some(items),
                                        ..Default::default()
                                    }
                                    .into(),
                                ),
                                ..Instance::new()
                            }),
                            ..Default::default()
                        })
                        .await
                        .map(|_operation| ())
                        .map_err(|e| {
                            tracing::error!(instance_name = %name, ?e, "Failed to create instance from template");
                            effect_error(e)
                        }),
                )
            }
            Effect::DeleteInstance { name, zone } => Response::Deleted(
                self.compute
                    .compute_instances_delete(ComputePeriodInstancesPeriodDeleteParams {
                        project: self.project_id.to_string(),
                        zone: zone.clone(),
                        instance: name.clone(),
                        ..Default::default()
                    })
                    .await
                    .map(|_operation| ())
                    .map_err(effect_error),
            ),
        }
    }
}

/// Drive a [`Handler`] to completion against real APIs.
pub async fn drive(deps: &Deps<'_>, cfg: &Config, event: &JobEvent) -> Outcome {
    let (mut handler, mut step) = Handler::start(cfg, event);
    loop {
        let effects = match step {
            Step::Done(outcome) => return outcome,
            Step::Do(effects) => effects,
        };

        for effect in &effects {
            info!(?effect, "Running effect");
        }
        let responses = join_all(effects.iter().map(|e| deps.run(e, event))).await;

        // A batch is always answered completely before the machine issues
        // its next one, so at most one response yields new effects.
        let mut next = Vec::new();
        for response in responses {
            match handler.resume(response) {
                Step::Done(outcome) => return outcome,
                Step::Do(effects) => next.extend(effects),
            }
        }
        if next.is_empty() {
            return Outcome::Failed {
                status: http::StatusCode::INTERNAL_SERVER_ERROR,
                message: "state machine stalled".to_string(),
            };
        }
        step = Step::Do(next);
    }
}

/// Handle one webhook event: run the state machine and map its outcome to an
/// HTTP result.
#[instrument(
    skip_all,
    fields(conclusion, job_id, repo_url, repository, run_attempt, run_id),
    err(Debug)
)]
pub async fn handle_event(
    deps: &Deps<'_>,
    webhook: &crate::webhook::WorkflowJobWebhook,
    event: &JobEvent,
) -> Result<(), Box<ErrorResponse>> {
    add_event_fields_to_span(webhook);

    let cfg = Config {
        region: deps.region.to_string(),
    };

    match drive(deps, &cfg, event).await {
        Outcome::Failed { status, message } => {
            tracing::error!(%status, message, "Workflow job handling failed");
            Err(Box::new((status, message).into()))
        }
        outcome => {
            info!(?outcome, "Workflow job handled");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use tracing_subscriber::EnvFilter;

    #[ignore] // Disabled test - run manually with `cargo test test_create_instance -- --ignored`
    #[tokio::test]
    async fn test_create_instance() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

        // Initialize tracing for test output
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .init();

        // Set up test environment variables (you'll need to set these manually)
        let project_id =
            env::var("GCP_PROJECT_ID").unwrap_or_else(|_| "finches-470422-builds".to_string());
        let region = env::var("GCP_REGION").unwrap_or_else(|_| "us-central1".to_string());

        let body = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/queued-payload.json"
        ));
        let deserializer = &mut serde_json::Deserializer::from_slice(body);
        let mock_event =
            serde_path_to_error::deserialize::<_, crate::webhook::WorkflowJobWebhook>(deserializer)
                .unwrap();
        let event = crate::webhook::job_event(&mock_event).expect("valid job event");

        let client = crate::compute::ComputeClient::new()
            .await
            .expect("Failed to create compute client");
        let github = crate::github::GithubClient::new();

        let deps = Deps {
            compute: &client,
            github: &github,
            project_id: &project_id,
            region: &region,
            github_token: "test-token",
            instance_template: "test-template",
        };

        handle_event(&deps, &mock_event, &event)
            .await
            .expect("instance creation failed");
    }
}
