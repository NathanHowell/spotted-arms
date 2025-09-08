use octocrab::models::webhook_events::payload::{
    WorkflowJobWebhookEventAction, WorkflowJobWebhookEventPayload,
};

#[test]
fn parse_in_progress_payload() {
    let payload_str = include_str!("fixtures/in-progress-payload.json");
    let payload: WorkflowJobWebhookEventPayload =
        serde_json::from_str(payload_str).expect("valid webhook payload");
    assert_eq!(payload.action, WorkflowJobWebhookEventAction::InProgress);
}

#[test]
fn parse_queued_payload() {
    let payload_str = include_str!("fixtures/queued-payload.json");
    let payload: WorkflowJobWebhookEventPayload =
        serde_json::from_str(payload_str).expect("valid webhook payload");
    assert_eq!(payload.action, WorkflowJobWebhookEventAction::Queued);
}
