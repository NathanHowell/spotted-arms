use crate::instance::{Deps, handle_event};
use crate::machine::{JobAction, JobEvent};
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
use serde_json::Value;
use tracing::field;
use tracing::{Instrument, Span, info, info_span, instrument};

#[derive(Deserialize)]
pub struct WorkflowJobWebhook {
    pub _sender: Option<Author>,
    pub repository: Repository,
    pub _organization: Option<Organization>,
    pub _installation: Option<EventInstallation>,
    #[serde(flatten)]
    pub payload: WorkflowJobWebhookEventPayload,
}

/// Extracts the fields the state machine needs from a `workflow_job` webhook.
/// Rejects payloads without job identifiers rather than defaulting them (a
/// default would make every malformed event collide on one instance name).
pub fn job_event(body: &WorkflowJobWebhook) -> Result<JobEvent, &'static str> {
    let job = &body.payload.workflow_job;
    let id = |key| job.get(key).and_then(Value::as_i64);

    Ok(JobEvent {
        action: match body.payload.action {
            WorkflowJobWebhookEventAction::Queued => JobAction::Queued,
            WorkflowJobWebhookEventAction::Completed => JobAction::Completed,
            _ => JobAction::Other,
        },
        repo_url: body.repository.url.clone(),
        run_id: id("run_id").ok_or("workflow_job.run_id missing")?,
        job_id: id("id").ok_or("workflow_job.id missing")?,
        labels: job
            .get("labels")
            .and_then(Value::as_array)
            .map(|labels| {
                labels
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        runner_name: job
            .get("runner_name")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// Handles incoming GitHub workflow job webhook events
#[instrument(skip_all, fields(body, event, delivery, labels), err(Debug))]
#[allow(clippy::result_large_err)] // ErrorResponse must be returned directly for IntoResponse
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

    let event = job_event(&body).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    span.record("labels", field::debug(&event.labels));

    let span = info_span!("workflow_job_event",
        action = ?body.payload.action,
        instance_name = %crate::utils::instance_name(event.run_id, event.job_id),
    );

    let deps = Deps {
        compute: state.compute_client.as_ref(),
        github: state.github_client.as_ref(),
        project_id: &state.project_id,
        region: &state.region,
        github_token: &state.token,
        instance_template: &state.instance_template,
    };

    handle_event(&deps, &body, &event)
        .instrument(span)
        .await
        .map_err(|e| *e)
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
