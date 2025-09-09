use std::env;

/// Gets the current GCP project ID from the environment
async fn get_project_id() -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    // Try environment variable first
    if let Ok(project_id) = env::var("GOOGLE_CLOUD_PROJECT") {
        return Ok(project_id);
    }

    if let Ok(project_id) = env::var("GCP_PROJECT") {
        return Ok(project_id);
    }

    // Fallback: try to get from metadata service
    let client = reqwest::Client::new();
    let response = client
        .get("http://metadata.google.internal/computeMetadata/v1/project/project-id")
        .header("Metadata-Flavor", "Google")
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await?;

    if response.status().is_success() {
        Ok(response.text().await?)
    } else {
        Err("Unable to determine project ID".into())
    }
}

/// Gets the current zone from environment or metadata service.
/// Returns an error if it cannot be determined.
async fn get_current_zone() -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    // Try environment variable first
    if let Ok(zone) = env::var("GOOGLE_CLOUD_ZONE") {
        return Ok(zone);
    }

    // Fallback: try to get from metadata service
    let client = reqwest::Client::new();
    let response = client
        .get("http://metadata.google.internal/computeMetadata/v1/instance/zone")
        .header("Metadata-Flavor", "Google")
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await?;

    if response.status().is_success() {
        let zone_path = response.text().await?;
        // Extract zone name from the full path (e.g., "projects/123/zones/us-central1-f" -> "us-central1-f")
        match zone_path.split('/').next_back() {
            Some(z) if !z.is_empty() => Ok(z.to_string()),
            _ => Err("Unable to determine zone from metadata response".into()),
        }
    } else {
        Err("Unable to determine zone".into())
    }
}

/// Gets the current GCP environment configuration (project ID, zone, and region) in parallel
pub async fn get_gcp_environment()
-> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
    // Run project ID and zone lookups in parallel
    let (project_result, zone_result) = tokio::join!(get_project_id(), get_current_zone());

    let project_id = project_result?;
    let zone = zone_result?;
    let region = zone_to_region(zone);

    Ok((project_id, region))
}

/// Gets the region from a zone (e.g., "us-central1-f" -> "us-central1")
fn zone_to_region(zone: String) -> String {
    // Remove the last character (zone suffix) to get the region
    zone.rsplit_once('-')
        .map(|(region, _)| region.to_string())
        .unwrap_or(zone)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zone_to_region_standard_zones() {
        // Test standard GCP zone naming convention
        assert_eq!(zone_to_region("us-central1-a".to_string()), "us-central1");
        assert_eq!(zone_to_region("us-central1-b".to_string()), "us-central1");
        assert_eq!(zone_to_region("us-central1-c".to_string()), "us-central1");
        assert_eq!(zone_to_region("us-central1-f".to_string()), "us-central1");

        assert_eq!(zone_to_region("us-east1-a".to_string()), "us-east1");
        assert_eq!(zone_to_region("us-west1-b".to_string()), "us-west1");
        assert_eq!(zone_to_region("europe-west1-c".to_string()), "europe-west1");
        assert_eq!(
            zone_to_region("asia-southeast1-a".to_string()),
            "asia-southeast1"
        );
    }

    #[test]
    fn test_zone_to_region_edge_cases() {
        // Test zone names without hyphens (should return the original string)
        assert_eq!(zone_to_region("invalid".to_string()), "invalid");
        assert_eq!(zone_to_region("region".to_string()), "region");

        // Test empty string
        assert_eq!(zone_to_region("".to_string()), "");

        // Test single character
        assert_eq!(zone_to_region("a".to_string()), "a");
    }

    #[test]
    fn test_zone_to_region_multiple_hyphens() {
        // Test zones with multiple hyphens (should split on the last one)
        assert_eq!(
            zone_to_region("us-central1-special-a".to_string()),
            "us-central1-special"
        );
        assert_eq!(
            zone_to_region("europe-west1-custom-zone-b".to_string()),
            "europe-west1-custom-zone"
        );
        assert_eq!(
            zone_to_region("multi-part-region-name-c".to_string()),
            "multi-part-region-name"
        );
    }

    #[test]
    fn test_zone_to_region_numeric_suffixes() {
        // Test zones with numeric suffixes
        assert_eq!(zone_to_region("us-central1-1".to_string()), "us-central1");
        assert_eq!(
            zone_to_region("europe-west2-123".to_string()),
            "europe-west2"
        );
    }

    #[test]
    fn test_zone_to_region_single_hyphen() {
        // Test zone with only one hyphen
        assert_eq!(zone_to_region("region-a".to_string()), "region");
        assert_eq!(zone_to_region("us-1".to_string()), "us");
    }

    #[test]
    fn test_zone_to_region_trailing_hyphen() {
        // Test zone ending with hyphen (edge case)
        assert_eq!(zone_to_region("us-central1-".to_string()), "us-central1");
        assert_eq!(zone_to_region("region-".to_string()), "region");
    }
}
