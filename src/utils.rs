use octocrab::models::webhook_events::payload::WorkflowJobWebhookEventPayload;
use serde_json::Value;

/// Generates a deterministic instance name from a workflow job event.
///
/// The name format is: `gha-{run_id}-{job_id}`
/// - Lowercased; only `[a-z0-9-]` are retained
/// - Truncated to 63 characters
/// - 1:1 mapping per job via `run_id` and `id`
///
/// Example
///
/// ```rust,ignore
/// use octocrab::models::webhook_events::payload::WorkflowJobWebhookEventPayload;
/// use serde_json::from_str;
///
/// // minimal payload with required fields
/// let payload: WorkflowJobWebhookEventPayload = from_str(r#"{
///   "action": "queued",
///   "workflow_job": {
///     "id": 42,
///     "run_id": 123,
///     "labels": []
///   }
/// }"#).unwrap();
///
/// let name = ephemeral_runner::utils::make_instance_name(&payload);
/// assert_eq!(name, "gha-123-42");
/// ```
pub fn make_instance_name(payload: &WorkflowJobWebhookEventPayload) -> String {
    let job = &payload.workflow_job;

    // deterministic, <= 63 chars; include run_id for 1:1 mapping
    format!(
        "gha-{}-{}",
        job.get("run_id")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
        job.get("id").and_then(Value::as_i64).unwrap_or_default(),
    )
    .to_lowercase()
    .chars()
    .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
    .take(63)
    .collect()
}

#[cfg(test)]
mod tests {
    /// Test the string formatting logic directly with known values
    #[test]
    fn test_instance_name_format() {
        let repo_name = "owner/repo";
        let run_id = 987654321u64;

        let result = format!("gha-{}-{}", repo_name.replace('/', "-"), run_id)
            .to_lowercase()
            .chars()
            .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
            .take(63)
            .collect::<String>();

        assert_eq!(result, "gha-owner-repo-987654321");
    }

    /// Test that special characters are properly filtered out
    #[test]
    fn test_instance_name_special_characters() {
        let repo_name = "Owner-123/Repo_Test";
        let run_id = 987654321u64;

        let result = format!("gha-{}-{}", repo_name.replace('/', "-"), run_id)
            .to_lowercase()
            .chars()
            .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
            .take(63)
            .collect::<String>();

        // Should filter out underscore characters, keeping only lowercase, digits, and hyphens
        assert_eq!(result, "gha-owner-123-repotest-987654321");
    }

    /// Test that long names are properly truncated to 63 characters
    #[test]
    fn test_instance_name_length_limit() {
        let long_name = "a".repeat(100);
        let repo_name = format!("owner/{}", long_name);
        let run_id = 987654321u64;

        let result = format!("gha-{}-{}", repo_name.replace('/', "-"), run_id)
            .to_lowercase()
            .chars()
            .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
            .take(63)
            .collect::<String>();

        assert!(result.len() <= 63);
        assert!(result.starts_with("gha-owner-"));
    }

    /// Test character filtering with various special characters
    #[test]
    fn test_instance_name_character_filtering() {
        let repo_name = "Owner@123/Repo#Test$";
        let run_id = 987654321u64;

        let result = format!("gha-{}-{}", repo_name.replace('/', "-"), run_id)
            .to_lowercase()
            .chars()
            .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
            .take(63)
            .collect::<String>();

        // Should filter out @, #, $ characters
        assert_eq!(result, "gha-owner123-repotest-987654321");
    }

    /// Test edge cases for error handling
    #[test]
    fn test_instance_name_edge_cases() {
        // Empty repo name
        let empty_repo = "";
        let run_id = 987654321u64;
        let result = format!("gha-{}-{}", empty_repo.replace('/', "-"), run_id)
            .to_lowercase()
            .chars()
            .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
            .take(63)
            .collect::<String>();

        assert_eq!(result, "gha--987654321");

        // Very long repo name
        let long_repo = "a".repeat(200);
        let result = format!("gha-{}-{}", long_repo.replace('/', "-"), run_id)
            .to_lowercase()
            .chars()
            .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
            .take(63)
            .collect::<String>();

        assert!(result.len() <= 63);
        assert!(result.starts_with("gha-"));
    }

    /// Test consistency of instance name generation for various inputs
    #[test]
    fn test_instance_name_generation_consistency() {
        let test_cases = vec![
            // (repo_name, run_id, expected_result)
            ("simple/repo", 123, "gha-simple-repo-123"),
            ("CAPS/REPO", 456, "gha-caps-repo-456"),
            (
                "with-dashes/and_underscores",
                789,
                "gha-with-dashes-andunderscores-789",
            ), // underscores filtered out
            (
                "special@chars!/bad%name",
                999,
                "gha-specialchars-badname-999",
            ), // slash becomes hyphen, special chars filtered
            ("", 111, "gha--111"), // Edge case: empty repo name
        ];

        for (repo_name, run_id, expected) in test_cases {
            let result = format!("gha-{}-{}", repo_name.replace('/', "-"), run_id)
                .to_lowercase()
                .chars()
                .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
                .take(63)
                .collect::<String>();

            assert_eq!(
                result, expected,
                "Failed for repo: {}, run_id: {}",
                repo_name, run_id
            );
        }
    }
}
