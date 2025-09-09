# Cloud Run Webhook + Runner Template (OpenTofu module)

This module deploys the Spotted Arms webhook to Cloud Run and provisions a GCE regional instance template used to launch short‑lived ARM64 GitHub Actions runners on demand.

- Cloud Run service exposes the webhook endpoint and is open to unauthenticated requests (required by GitHub webhooks)
- Regional Instance Template defines a COS ARM64 VM with a startup script that configures Docker and a systemd unit for the ephemeral runner
- Secrets from Secret Manager are mapped into container environment variables

## Usage (via GitHub source)

```hcl
module "gha_webhook" {
  source = "github.com/NathanHowell/spotted-arms//modules/cloud-run?ref=main" # pin to a tag/sha in production

  # Cloud Run
  location                     = "us-central1"
  service_name                 = "spotted-arms"
  container_image              = "us-central1-docker.pkg.dev/PROJECT/REPO/spotted-arms:TAG"
  webhook_service_account_email = "webhook-sa@PROJECT.iam.gserviceaccount.com"

  # Runners
  vm_service_account_email   = "gha-runner@PROJECT.iam.gserviceaccount.com"
  vm_subnetwork_self_link    = "projects/PROJECT/regions/us-central1/subnetworks/DEFAULT"

  # Map Secret Manager secret names to env vars inside the service
  secrets = {
    GITHUB_CREDENTIALS = "github-credentials"  # JSON: {"token":"...","secret":"..."}
    INSTANCE_TEMPLATE  = "instance-template"   # Name of the GCE region instance template
  }

  # Optional tuning
  cpu_limit       = "1000m"
  memory_limit    = "512Mi"
  cpu_request     = "250m"
  memory_request  = "256Mi"
  min_scale       = 0
  max_scale       = 10
}
```

Outputs:
- `service_url` – Cloud Run URL (use as the GitHub webhook URL, typically `${service_url}/webhook`)
- `service_name` – Deployed service name
- `service_location` – Region of the service

## Prerequisites

- Enable APIs: Cloud Run, Compute Engine, Secret Manager, Artifact Registry
- Service accounts and minimal roles (example – adjust to your security model):
  - Webhook SA: read Secret Manager (`roles/secretmanager.secretAccessor`), manage Compute (create/delete instances) if needed by the service
  - Runner VM SA: pull container registry as needed, write logs, etc.
- Secret Manager entries in the same project:
  - `github-credentials`: JSON with `token` and `secret`
  - `instance-template`: string value of the instance template name (see below)

## Notes

- The instance template created by this module is named `c4a-standard-1` and uses COS ARM64. Use that value for the `INSTANCE_TEMPLATE` secret unless you customize the module.
- The application auto‑discovers project/region on Cloud Run. `GITHUB_CREDENTIALS` and `INSTANCE_TEMPLATE` must be provided via secrets.
- Exposes `/ping` for health checks and `/webhook` for GitHub webhooks.

## Configure GitHub Webhook

- Set payload URL to `${service_url}/webhook`
- Content type: `application/json`
- Secret: the same `secret` value inside `GITHUB_CREDENTIALS`
- Events: select `workflow_job`

## Providers

- Tested with the Google provider for OpenTofu/Terraform. Example:

```hcl
terraform {
  required_version = ">= 1.6.0"
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = ">= 5.0"
    }
  }
}

provider "google" {
  project = "PROJECT"
  region  = "us-central1"
}
```
