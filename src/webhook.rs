use crate::instance::{create_instance, delete_instance};
use crate::utils::make_instance_name;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::ErrorResponse;
use axum_github_webhook_extract::GithubEvent;
use octocrab::models::orgs::Organization;
use octocrab::models::webhook_events::EventInstallation;
use octocrab::models::webhook_events::payload::{
    WorkflowJobWebhookEventAction, WorkflowJobWebhookEventPayload,
};
use octocrab::models::{Author, Repository};
use serde::Deserialize;
use std::collections::HashSet;
use tracing::field;
use tracing::{Instrument, Span, info, info_span, instrument};

const REQUIRED_LABELS: &[&str] = &["linux", "self-hosted", "ARM64"];

/// Checks if the job has the required labels for GCP ARM64 runners
fn has_required_labels<'a>(labels: impl IntoIterator<Item = &'a String>) -> bool {
    let labels = labels
        .into_iter()
        .map(String::as_ref)
        .collect::<HashSet<&str>>();

    REQUIRED_LABELS
        .iter()
        .all(|&required| labels.contains(required))
}

#[derive(Deserialize)]
pub struct WorkflowJobWebhook {
    pub _sender: Option<Author>,
    pub repository: Repository,
    pub _organization: Option<Organization>,
    pub _installation: Option<EventInstallation>,
    #[serde(flatten)]
    pub payload: WorkflowJobWebhookEventPayload,
}

/// Handles incoming GitHub workflow job webhook events
#[instrument(skip_all, fields(body, event, delivery, labels), err(Debug))]
pub async fn handle_workflow_job_event(
    headers: HeaderMap,
    State(state): State<crate::server::AppState>,
    GithubEvent(body): GithubEvent<WorkflowJobWebhook>,
) -> Result<(), ErrorResponse> {
    let span = Span::current();

    let event_type = headers
        .get("X-GitHub-Event")
        .ok_or((StatusCode::BAD_REQUEST, "missing X-GitHub-Event header"))
        .and_then(|v| {
            v.to_str()
                .map_err(|_| (StatusCode::BAD_REQUEST, "invalid X-GitHub-Event header"))
        })?;

    span.record("event", event_type);

    let delivery = headers
        .get("X-GitHub-Delivery")
        .and_then(|v| v.to_str().ok());

    span.record("delivery", delivery);

    if event_type != "workflow_job" {
        info!(event_type, "Ignoring non-workflow_job event");
        return Ok(());
    }

    let workflow_job = &body.payload.workflow_job;
    let labels = &workflow_job
        .get("labels")
        .unwrap_or_default()
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect::<HashSet<_>>();

    span.record("labels", field::debug(labels));

    // Check if the job has required labels before creating instance
    if !has_required_labels(labels) {
        info!(
            job.labels = ?labels,
            required.labels = ?REQUIRED_LABELS,
            "Ignoring job without required labels",
        );
        return Ok(());
    }

    let instance_name = make_instance_name(&body.payload);

    let span = info_span!("workflow_job_event",
        action = ?body.payload.action,
        instance_name = %instance_name
    );

    async move {
        match body.payload.action {
            WorkflowJobWebhookEventAction::Queued => {
                info!("Processing queued workflow job");
                create_instance(
                    state.compute_client.as_ref(),
                    state.github_client.as_ref(),
                    &state.project_id,
                    &state.region,
                    &state.token,
                    instance_name.as_str(),
                    &body,
                )
                .await
            }
            WorkflowJobWebhookEventAction::Completed => {
                info!("Processing completed workflow job");
                delete_instance(
                    state.compute_client.as_ref(),
                    &state.project_id,
                    &state.region,
                    instance_name.as_str(),
                    &body,
                )
                .await
            }
            _ => {
                info!(?body.payload.action, "Ignoring workflow job event");
                Ok(())
            }
        }
    }
    .instrument(span)
    .await
}

#[cfg(test)]
mod tests {
    use octocrab::models::webhook_events::payload::{
        WorkflowJobWebhookEventAction, WorkflowJobWebhookEventPayload,
    };

    #[test]
    fn parse_log_payload_as_workflow_job_event() {
        // Raw log line containing the JSON payload (truncated lines kept exactly as in the log).
        let payload = include_str!("../tests/fixtures/in-progress-payload.json");

        let payload = serde_json::from_str::<WorkflowJobWebhookEventPayload>(payload)
            .expect("valid webhook payload");

        assert_eq!(payload.action, WorkflowJobWebhookEventAction::InProgress);
    }
}
