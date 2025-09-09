resource "google_compute_region_instance_template" "template" {
  name        = "c4a-standard-1"
  description = "Github Actions Runner Template"
  region      = "us-central1"

  machine_type   = "c4a-standard-1"
  can_ip_forward = false

  confidential_instance_config {
    enable_confidential_compute = false
  }

  disk {
    auto_delete = true
    boot        = true
    device_name = "${var.service_name}-template"
    mode        = "READ_WRITE"
    type        = "PERSISTENT"

    disk_size_gb = 100
    disk_type    = "hyperdisk-balanced"
    source_image = "projects/cos-cloud/global/images/family/cos-arm64-stable"
  }

  network_interface {
    subnetwork = var.vm_subnetwork_self_link
    nic_type   = "GVNIC"
    stack_type = "IPV4_IPV6"

    access_config {
      # sadly Github Actions requires IPv4 as of 2025-09-03
      network_tier = "STANDARD"
    }

    ipv6_access_config {
      network_tier = "PREMIUM"
    }
  }

  reservation_affinity {
    type = "NO_RESERVATION"
  }

  scheduling {
    automatic_restart           = false
    on_host_maintenance         = "TERMINATE"
    provisioning_model          = "SPOT"
    preemptible                 = true
    instance_termination_action = "DELETE"

    max_run_duration {
      seconds = 3600
      nanos   = 0
    }
  }

  service_account {
    email = var.vm_service_account_email
    scopes = [
      "https://www.googleapis.com/auth/cloud-platform",
    ]
  }

  shielded_instance_config {
    enable_integrity_monitoring = true
    enable_secure_boot          = true
    enable_vtpm                 = true
  }

  metadata = {
    enable-oslogin     = "TRUE"
    serial-port-enable = "TRUE"

    startup-script = file("${path.module}/cloud-init.py")
  }

  tags = ["allow-iap-ssh"]
}