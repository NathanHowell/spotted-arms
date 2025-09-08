# Ephemeral Runner for GitHub Actions on Google Compute Engine

A small Rust service that listens for GitHub `workflow_job` webhooks and provisions ephemeral self‑hosted runners on Google Compute Engine using a region instance template. When jobs complete, the service deletes the instance.

## Features
- Axum HTTP server with webhook verification
- JIT runner configuration via GitHub API
- GCE instance create/delete from a region instance template
- Deterministic zone selection within a region
- Structured JSON logging and OpenTelemetry (Cloud Trace) export
- Health and ping endpoints

## Endpoints
- `POST /webhook` — GitHub webhook receiver for `workflow_job` events
- `GET /ping` — simple liveness probe (returns `pong`)
- `POST /health_check` — returns JSON status and request headers

## Requirements
- Rust toolchain (1.75+ recommended)
- A Google Cloud project and service account with permissions:
  - `compute.instances.insert`
  - `compute.instances.delete`
  - `compute.regionInstanceTemplates.get`
- A pre‑created GCE region instance template for your runner image
- A GitHub App or token able to call the JIT config endpoint with repository admin permissions for Actions runners (or equivalent)

## Configuration
This service intentionally avoids baked‑in defaults. Provide configuration via environment variables or GCP metadata.

### Required
- `PORT` — TCP port for the HTTP server (e.g., `3000`).
- `GITHUB_CREDENTIALS` — JSON document with your GitHub token and webhook secret, for example:

  ```json
  { "token": "ghp_xxx", "secret": "webhook-shared-secret" }
  ```

- `INSTANCE_TEMPLATE` — Name of the GCE region instance template to use.

### Project/Location
- Project ID: either `GOOGLE_CLOUD_PROJECT` or `GCP_PROJECT`, or discoverable via the GCE metadata server.
- Zone/Region:
  - `GOOGLE_CLOUD_ZONE` (e.g., `us-central1-f`), or discoverable via metadata.
  - Region is derived from the zone suffix (e.g., `us-central1`).

### Telemetry
- `PROJECT_ID` — If set, used by the Cloud Trace exporter; otherwise falls back to GCP metadata discovery as above.

### GitHub filtering
- Jobs must include all required labels to be processed: `linux`, `self-hosted`, `ARM64`.

### Region support
- Currently, instance creation only supports the `us-central1` region. If your zone/region differs, the request is rejected. Zone within the region is selected deterministically per instance.

## Running locally
1. Export the required variables:

   ```bash
   export PORT=3000
   export INSTANCE_TEMPLATE=c4a-standard-1   # example
   export GITHUB_CREDENTIALS='{"token":"<token>","secret":"<secret>"}'
   export GOOGLE_CLOUD_PROJECT=<project>      # or ensure metadata is available
   export GOOGLE_CLOUD_ZONE=us-central1-f
   ```

2. Build and run:

   ```bash
   cargo run --bin ephemeral-runner
   ```

3. Send a test webhook payload (ensure the signature/secret matches) to `http://localhost:3000/webhook`.

## Deploying
- Run as a service on GCE/GKE/Cloud Run with the above environment.
- Ensure the service account has the Compute and Trace permissions listed.
- Configure your GitHub App webhook to point at `https://<your-host>/webhook` and set the same shared secret used in `GITHUB_CREDENTIALS`.

## How it works
- Webhook handler validates the request and inspects `workflow_job` events.
- If labels match, it derives an instance name and requests:
  - GitHub JIT config for the runner name
  - Region instance template metadata from GCE
- It injects the JIT config as instance metadata and calls `instances.insert`.
- On `workflow_job.completed`, it computes the same zone and calls `instances.delete`.

## Troubleshooting
- Missing `PORT` → process exits on startup.
- Missing `GITHUB_CREDENTIALS` or malformed JSON → startup error.
- Missing `INSTANCE_TEMPLATE` → request handling returns 500 with clear error and logs.
- Unable to determine project/zone → metadata call errors if env not set.
- Region not `us-central1` → request rejected; set a `us-central1-*` zone.

## Development
- Build: `cargo build`
- Check: `cargo check`
- Test: `cargo test`

Contributions and improvements welcome!
