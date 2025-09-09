resource "google_cloud_run_service" "service" {
  name     = var.service_name
  location = var.location

  template {
    metadata {
      annotations = {
        "autoscaling.knative.dev/minScale"         = tostring(var.min_scale)
        "autoscaling.knative.dev/maxScale"         = tostring(var.max_scale)
        "run.googleapis.com/execution-environment" = "gen1"
        "run.googleapis.com/cpu-throttling"        = "true"
      }
    }

    spec {
      service_account_name = var.webhook_service_account_email

      containers {
        image = var.container_image

        ports {
          container_port = 8080
          name           = "h2c" # Enable HTTP/2 cleartext
        }

        # Health check configuration
        startup_probe {
          http_get {
            path = "/ping"
            port = 8080
          }
          initial_delay_seconds = 1
          timeout_seconds       = 1
          period_seconds        = 2
          failure_threshold     = 10
        }

        liveness_probe {
          http_get {
            path = "/ping"
            port = 8080
          }
          initial_delay_seconds = 30
          timeout_seconds       = 5
          period_seconds        = 30
          failure_threshold     = 3
        }

        env {
          name  = "PROJECT_ID"
          value = data.google_client_config.default.project
        }

        dynamic "env" {
          for_each = var.secrets
          content {
            name = env.key
            value_from {
              secret_key_ref {
                name = env.value
                key  = "latest"
              }
            }
          }
        }

        resources {
          limits = {
            cpu    = var.cpu_limit
            memory = var.memory_limit
          }
          requests = {
            cpu    = var.cpu_request
            memory = var.memory_request
          }
        }
      }

      container_concurrency = 1000
      timeout_seconds       = 30
    }
  }

  traffic {
    percent         = 100
    latest_revision = true
  }

  autogenerate_revision_name = true
}

# Get the current project ID from the provider
data "google_client_config" "default" {}

# Allow unauthenticated invocations (REQUIRED for GitHub webhooks)
resource "google_cloud_run_service_iam_binding" "public_access" {
  location = google_cloud_run_service.service.location
  service  = google_cloud_run_service.service.name
  role     = "roles/run.invoker"
  members  = ["allUsers"]
}
