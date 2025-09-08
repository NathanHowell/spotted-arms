use crate::compute::{ComputeApi, ComputeError};
use crate::github::GithubApi;
use axum::response::ErrorResponse;
use gcloud_sdk::google_rest_apis::compute_v1;
use gcloud_sdk::google_rest_apis::compute_v1::Instance;
use gcloud_sdk::google_rest_apis::compute_v1::instances_api::{
    ComputePeriodInstancesPeriodDeleteParams, ComputePeriodInstancesPeriodInsertParams,
};
use gcloud_sdk::google_rest_apis::compute_v1::region_instance_templates_api::ComputePeriodRegionInstanceTemplatesPeriodGetParams;
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use tracing::{Span, field, info, instrument};

// Supported zones for us-central1 region
const US_CENTRAL1_ZONES: &[&str] = &[
    "us-central1-a",
    "us-central1-b",
    "us-central1-c",
    "us-central1-f",
];

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

/// Deterministically selects a zone based on instance name hash
fn select_zone_for_region(region: &str, instance_name: &str) -> Result<String, ErrorResponse> {
    if region != "us-central1" {
        tracing::error!(
            "Unsupported region: {}. Only us-central1 is currently supported.",
            region
        );
        return Err(ErrorResponse::from(axum::http::StatusCode::BAD_REQUEST));
    }

    let mut hasher = DefaultHasher::new();
    instance_name.hash(&mut hasher);
    let hash = hasher.finish();

    let zone_index = (hash as usize) % US_CENTRAL1_ZONES.len();
    let selected_zone = US_CENTRAL1_ZONES[zone_index];

    tracing::debug!(
        "Selected zone {} for instance {} in region {}",
        selected_zone,
        instance_name,
        region
    );

    Ok(selected_zone.to_string())
}

/// Creates a new compute instance from a template for the given workflow job
#[instrument(
    skip(api, github, event, github_token),
    fields(job_id, repo_url, repository, run_attempt, run_id),
    err(Debug)
)]
pub async fn create_instance(
    api: &dyn ComputeApi,
    github: &dyn GithubApi,
    project_id: &str,
    region: &str,
    github_token: &str,
    instance_template: &str,
    instance_name: &str,
    event: &crate::webhook::WorkflowJobWebhook,
) -> Result<(), ErrorResponse> {
    add_event_fields_to_span(event);

    let repo_url = event.repository.url.clone();
    if repo_url.host_str() != Some("api.github.com") {
        tracing::error!(
            repo_url = display(repo_url),
            "Unexpected repository URL format"
        );
        return Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "Invalid repository URL format",
        )
            .into());
    }

    // Extract runner name and labels from the event payload
    let runner_name = instance_name; // Use instance name as runner name
    let payload = &event.payload;
    let labels = payload
        .workflow_job
        .get("labels")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();

    // Use provided instance template
    let template_name = instance_template.to_string();

    // Generate JIT config and fetch template metadata concurrently
    let (jit_config, template_metadata) = tokio::try_join!(
        async {
            github
                .generate_jit_config(&repo_url, github_token, &runner_name, &labels)
                .await
                .map_err(|e| -> ErrorResponse {
                    tracing::error!(?e, "Failed to generate JIT config");
                    (
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        "jit config failed",
                    )
                        .into()
                })
        },
        async {
            api.compute_region_instance_templates_get(
                ComputePeriodRegionInstanceTemplatesPeriodGetParams {
                    project: project_id.to_string(),
                    region: region.to_string(),
                    instance_template: template_name.to_string(),
                    fields: Some("properties.metadata".to_string()),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| -> ErrorResponse {
                tracing::error!(?e, "Failed to get instance template metadata");
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "templates get failed",
                )
                    .into()
            })
        }
    )?;

    info!(
        instance_name,
        labels = ?event.payload.workflow_job.get("labels"),
        "Creating instance from template for job",
    );

    // Select zone deterministically based on instance name
    let zone = select_zone_for_region(region, instance_name)?;

    // Use the preexisting instance template
    let source_instance_template = format!(
        "projects/{}/regions/{}/instanceTemplates/{}",
        project_id, region, template_name
    );

    info!(source_instance_template, zone, "Using instance template");

    // there isn't a way to merge metadata items, so we have to do it manually
    let mut metadata = template_metadata
        .properties
        .and_then(|p| p.metadata)
        .and_then(|m| m.items)
        .unwrap_or_default();
    metadata.push(compute_v1::MetadataItemsInner {
        key: Some("JIT_CONFIG".to_string()),
        value: Some(jit_config),
    });

    let request = ComputePeriodInstancesPeriodInsertParams {
        project: project_id.to_string(),
        zone: zone.clone(),
        source_instance_template: Some(source_instance_template),
        instance: Some(Instance {
            name: Some(instance_name.to_string()),
            metadata: Some(
                compute_v1::Metadata {
                    items: Some(metadata),
                    ..Default::default()
                }
                .into(),
            ),
            ..Instance::new()
        }),
        ..Default::default()
    };

    match api.compute_instances_insert(request).await {
        Ok(_operation) => {
            info!(
                instance_name,
                zone, "Successfully initiated instance creation from template",
            );
            Ok(())
        }
        Err(e) => {
            tracing::error!(instance_name, ?e, "Failed to create instance from template",);

            Err((
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("{e:?}"),
            )
                .into())
        }
    }
}

/// Deletes the compute instance for the given workflow job
#[instrument(
    skip(api, event),
    fields(conclusion, job_id, repo_url, repository, run_attempt, run_id),
    err(Debug)
)]
pub async fn delete_instance(
    api: &dyn ComputeApi,
    project_id: &str,
    region: &str,
    instance_name: &str,
    event: &crate::webhook::WorkflowJobWebhook,
) -> Result<(), ErrorResponse> {
    add_event_fields_to_span(event);

    info!(instance_name, "Deleting instance");

    // Select the same zone that was used for creation
    let zone = select_zone_for_region(region, instance_name)?;

    match api
        .compute_instances_delete(ComputePeriodInstancesPeriodDeleteParams {
            project: project_id.to_string(),
            zone: zone.clone(),
            instance: instance_name.to_string(),
            ..Default::default()
        })
        .await
    {
        Ok(_) => {
            info!(
                instance_name,
                zone, "Successfully initiated instance deletion"
            );
        }
        Err(ComputeError::NotFound) => {
            info!(
                instance_name,
                zone, "Instance not found in zone (may have already been deleted)"
            );
        }
        Err(other) => {
            tracing::error!(instance_name, ?other, "Failed to delete instance");
            return Err((
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("{other:?}"),
            )
                .into());
        }
    }

    Ok(())
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

        // Create a mock WorkflowJobWebhook using JSON deserialization
        // This is much cleaner than trying to construct the struct manually
        let body = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/queued-payload.json"
        ));
        let deserializer = &mut serde_json::Deserializer::from_slice(body);
        let mock_event =
            serde_path_to_error::deserialize::<_, crate::webhook::WorkflowJobWebhook>(deserializer)
                .unwrap();

        let client = crate::compute::ComputeClient::new()
            .await
            .expect("Failed to create compute client");
        let github = crate::github::GithubClient::new();

        let instance_name = "test-runner-12345";
        let github_token = "test-token";

        println!("Creating instance with parameters:");
        println!("  Project ID: {}", project_id);
        println!("  Region: {}", region);
        println!("  Instance Name: {}", instance_name);
        println!(
            "  Job Labels: {:?}",
            mock_event.payload.workflow_job.get("labels")
        );

        // Call the function under test with correct parameter order
        let result = create_instance(
            &client,
            &github,
            &project_id,
            &region,
            &github_token,
            "test-template",
            instance_name,
            &mock_event,
        )
        .await;

        match result {
            Ok(()) => {
                println!("✅ Instance creation initiated successfully!");
                println!("Check the Google Cloud Console to verify the instance was created.");
            }
            Err(e) => {
                println!("❌ Instance creation failed: {:?}", e);
                panic!("Test failed with error: {:?}", e);
            }
        }
    }
}
