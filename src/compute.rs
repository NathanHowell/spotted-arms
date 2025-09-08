use gcloud_sdk::GoogleRestApi;
use gcloud_sdk::google_rest_apis::compute_v1;
use gcloud_sdk::google_rest_apis::compute_v1::instances_api::{
    ComputePeriodInstancesPeriodDeleteParams, ComputePeriodInstancesPeriodInsertParams,
    compute_instances_delete, compute_instances_insert,
};
use gcloud_sdk::google_rest_apis::compute_v1::region_instance_templates_api::{
    ComputePeriodRegionInstanceTemplatesPeriodGetParams, compute_region_instance_templates_get,
};
use std::future::Future;
use std::pin::Pin;
use thiserror::Error;
use tracing::instrument;

#[derive(Debug, Error)]
pub enum ComputeError {
    #[error("resource not found")]
    NotFound,
    #[error("compute error: {0}")]
    Other(String),
}

/// Abstraction over the subset of Google Compute API functionality we use.
pub trait ComputeApi: Send + Sync {
    /// Low-level region instance templates get
    fn compute_region_instance_templates_get(
        &self,
        params: ComputePeriodRegionInstanceTemplatesPeriodGetParams,
    ) -> Pin<Box<dyn Future<Output = Result<compute_v1::InstanceTemplate, ComputeError>> + Send>>;

    /// Low-level instances insert
    fn compute_instances_insert(
        &self,
        params: ComputePeriodInstancesPeriodInsertParams,
    ) -> Pin<Box<dyn Future<Output = Result<compute_v1::Operation, ComputeError>> + Send>>;

    /// Low-level instances delete
    fn compute_instances_delete(
        &self,
        params: ComputePeriodInstancesPeriodDeleteParams,
    ) -> Pin<Box<dyn Future<Output = Result<compute_v1::Operation, ComputeError>> + Send>>;
}

/// Default GCP-backed implementation that wraps GoogleRestApi and builds config per call.
pub struct ComputeClient {
    inner: std::sync::Arc<GoogleRestApi>,
}

impl ComputeClient {
    pub async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let inner = GoogleRestApi::new().await?;
        Ok(Self {
            inner: std::sync::Arc::new(inner),
        })
    }
}

impl ComputeApi for ComputeClient {
    #[instrument(skip(self), err(Debug))]
    fn compute_region_instance_templates_get(
        &self,
        params: ComputePeriodRegionInstanceTemplatesPeriodGetParams,
    ) -> Pin<Box<dyn Future<Output = Result<compute_v1::InstanceTemplate, ComputeError>> + Send>>
    {
        let inner = self.inner.clone();
        Box::pin(async move {
            let config = inner
                .create_google_compute_v1_config()
                .await
                .map_err(|e| ComputeError::Other(e.to_string()))?;
            compute_region_instance_templates_get(&config, params)
                .await
                .map_err(|e| ComputeError::Other(e.to_string()))
        })
    }

    #[instrument(skip(self), err(Debug))]
    fn compute_instances_insert(
        &self,
        params: ComputePeriodInstancesPeriodInsertParams,
    ) -> Pin<Box<dyn Future<Output = Result<compute_v1::Operation, ComputeError>> + Send>> {
        let inner = self.inner.clone();
        Box::pin(async move {
            let config = inner
                .create_google_compute_v1_config()
                .await
                .map_err(|e| ComputeError::Other(e.to_string()))?;
            compute_instances_insert(&config, params)
                .await
                .map_err(|e| ComputeError::Other(e.to_string()))
        })
    }

    #[instrument(skip(self), err(Debug))]
    fn compute_instances_delete(
        &self,
        params: ComputePeriodInstancesPeriodDeleteParams,
    ) -> Pin<Box<dyn Future<Output = Result<compute_v1::Operation, ComputeError>> + Send>> {
        let inner = self.inner.clone();
        Box::pin(async move {
            let config = inner
                .create_google_compute_v1_config()
                .await
                .map_err(|e| ComputeError::Other(e.to_string()))?;
            compute_instances_delete(&config, params)
                .await
                .map_err(|e| {
                    if let compute_v1::Error::ResponseError(resp) = &e {
                        if resp.status == reqwest::StatusCode::NOT_FOUND {
                            return ComputeError::NotFound;
                        }
                    }
                    ComputeError::Other(e.to_string())
                })
        })
    }
}
