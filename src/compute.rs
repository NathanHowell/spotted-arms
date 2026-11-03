use google_cloud_compute_v1::client::{Instances, RegionInstanceTemplates};
use google_cloud_compute_v1::model::{Instance, Metadata, metadata};
use std::future::Future;
use std::pin::Pin;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ComputeError {
    #[error("resource not found")]
    NotFound,
    #[error("compute error: {0}")]
    Other(String),
}

/// Boxed future returned by [`ComputeApi`] methods, so the trait stays object safe.
pub type ComputeFuture<T> = Pin<Box<dyn Future<Output = Result<T, ComputeError>> + Send>>;

/// Abstraction over the subset of Google Compute API functionality we use.
pub trait ComputeApi: Send + Sync {
    /// Metadata `(key, value)` pairs of a regional instance template.
    fn get_template_metadata(
        &self,
        project: &str,
        region: &str,
        template: &str,
    ) -> ComputeFuture<Vec<(String, String)>>;

    /// Create instance `name` in `zone` from `source_instance_template` (a
    /// full resource path), with `metadata` as its complete metadata list.
    fn insert_instance(
        &self,
        project: &str,
        zone: &str,
        source_instance_template: &str,
        name: &str,
        metadata: &[(String, String)],
    ) -> ComputeFuture<()>;

    /// Delete instance `name` in `zone`.
    fn delete_instance(&self, project: &str, zone: &str, name: &str) -> ComputeFuture<()>;
}

/// Default GCP-backed implementation on the official Google Cloud Rust SDK.
pub struct ComputeClient {
    instances: Instances,
    templates: RegionInstanceTemplates,
}

impl ComputeClient {
    pub async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            instances: Instances::builder().build().await?,
            templates: RegionInstanceTemplates::builder().build().await?,
        })
    }
}

fn map_error(e: google_cloud_compute_v1::Error) -> ComputeError {
    if e.http_status_code() == Some(404) {
        return ComputeError::NotFound;
    }
    ComputeError::Other(e.to_string())
}

impl ComputeApi for ComputeClient {
    fn get_template_metadata(
        &self,
        project: &str,
        region: &str,
        template: &str,
    ) -> ComputeFuture<Vec<(String, String)>> {
        let request = self
            .templates
            .get()
            .set_project(project)
            .set_region(region)
            .set_instance_template(template);
        Box::pin(async move {
            let template = request.send().await.map_err(map_error)?;
            Ok(template
                .properties
                .and_then(|p| p.metadata)
                .map(|m| m.items)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|item| Some((item.key?, item.value.unwrap_or_default())))
                .collect())
        })
    }

    fn insert_instance(
        &self,
        project: &str,
        zone: &str,
        source_instance_template: &str,
        name: &str,
        metadata: &[(String, String)],
    ) -> ComputeFuture<()> {
        let items = metadata.iter().map(|(key, value)| {
            metadata::Items::new()
                .set_key(key.as_str())
                .set_value(value.as_str())
        });
        let request = self
            .instances
            .insert()
            .set_project(project)
            .set_zone(zone)
            .set_source_instance_template(source_instance_template)
            .set_body(
                Instance::new()
                    .set_name(name)
                    .set_metadata(Metadata::new().set_items(items)),
            );
        Box::pin(async move {
            request.send().await.map_err(map_error)?;
            Ok(())
        })
    }

    fn delete_instance(&self, project: &str, zone: &str, name: &str) -> ComputeFuture<()> {
        let request = self
            .instances
            .delete()
            .set_project(project)
            .set_zone(zone)
            .set_instance(name);
        Box::pin(async move {
            request.send().await.map_err(map_error)?;
            Ok(())
        })
    }
}
