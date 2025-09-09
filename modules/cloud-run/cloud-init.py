#!/usr/bin/env python3
"""
GitHub Actions runner startup script for GCE instances.

This script is intended to be used as a cloud-init style startup script
for the runner VM. It performs the following high-level steps:

- Detects the current GCP project ID and region from the metadata server.
- Prepares directories for the runner workdir, persisting them on the
  stateful partition and bind-mounting into their runtime locations.
- Configures Docker (auth and daemon.json) to use an Artifact Registry
  virtual repository in the current region as a mirror, then reloads Docker.
- Creates and enables a systemd unit that runs the GitHub Actions runner
  in a container, and powers off the VM when the job completes.

Notes
- This script assumes access to the GCE metadata server
  (http://metadata.google.internal) and that Docker is installed.
- It targets a minimal, ephemeral lifecycle: run one job and then power off.
"""

import base64
import json
import logging
import os
import subprocess
import sys
import urllib.request
from http.client import HTTPResponse
from pathlib import Path
from typing import Any, Dict

logger = logging.getLogger(__name__)


class GCPJSONFormatter(logging.Formatter):
    """JSON formatter optimized for GCP Cloud Logging.

    Formats Python log records into a structure that Cloud Logging understands
    so severity, source location, and timestamps are parsed and indexed.
    """

    def format(self, record: logging.LogRecord) -> str:  # type: ignore[override]
        # Build the log entry following GCP Cloud Logging structure
        log_entry: Dict[str, Any] = {
            "severity": record.levelname or "DEFAULT",
            "message": record.getMessage(),
            "timestamp": {
                "seconds": int(record.created),
                "nanos": int((record.created % 1) * 1_000_000_000),
            },
            "sourceLocation": {
                "file": record.pathname,
                "line": str(record.lineno),
                "function": record.funcName,
            },
            "labels": {
                "component": "github-actions-runner",
                "module": record.module,
            },
        }

        # Add exception information if present
        if record.exc_info:
            log_entry["exception"] = self.formatException(record.exc_info)

        # Attach arbitrary extras if provided by the caller
        if hasattr(record, "extra_fields"):
            log_entry.update(record.extra_fields)  # type: ignore[arg-type]

        return json.dumps(log_entry)


def get_project_id() -> str:
    """Return the current GCP project ID via the metadata server."""
    # The metadata server is only reachable from within a GCE VM.
    req = urllib.request.Request(
        "http://metadata.google.internal/computeMetadata/v1/project/project-id",
        headers={"Metadata-Flavor": "Google"},
    )
    response: HTTPResponse
    with urllib.request.urlopen(req, timeout=10) as response:
        project_id = response.read().decode("utf-8").strip()
        logger.debug(f"Retrieved project ID: {project_id}")
        return project_id


def get_region() -> str:
    """Return the region derived from the VM's zone via the metadata server.

    The metadata value has the format:
      projects/<num>/zones/<region>-<zone-suffix>
    This function returns just the `<region>` portion.
    """
    req = urllib.request.Request(
        "http://metadata.google.internal/computeMetadata/v1/instance/zone",
        headers={"Metadata-Flavor": "Google"},
    )
    response: HTTPResponse
    with urllib.request.urlopen(req, timeout=10) as response:
        zone_path = response.read().decode("utf-8").strip()
        logger.debug(f"Retrieved zone path: {zone_path}")
        # Zone format: projects/12345/zones/us-central1-a
        zone = zone_path.partition("/zones/")[-1]
        region = zone.rsplit("-", 1)[0]
        return region


def get_jitconfig() -> str:
    """Return the GitHub runner JIT configuration from instance metadata.

    The `JIT_CONFIG` attribute is expected to be attached to the instance
    and consumed by `actions/runner` for just-in-time configuration.
    """
    req = urllib.request.Request(
        "http://metadata.google.internal/computeMetadata/v1/instance/attributes/JIT_CONFIG",
        headers={"Metadata-Flavor": "Google"},
    )
    response: HTTPResponse
    with urllib.request.urlopen(req, timeout=10) as response:
        return response.read().decode("utf-8")


def fetch_secret(secret_name: str, project_id: str, access_token: str) -> Dict[str, Any]:
    """Fetch and return a secret JSON payload from Secret Manager.

    Parameters
    - secret_name: Secret resource name within the project.
    - project_id: GCP project ID where the secret resides.
    - access_token: OAuth2 Bearer token with `secretmanager.versions.access`.

    Returns
    - Parsed JSON value stored in the secret's latest version.

    Note: This helper is currently unused by the flow but is handy for
    retrieving JSON secrets if needed in the future.
    """
    url = (
        f"https://secretmanager.googleapis.com/v1/projects/{project_id}/secrets/"
        f"{secret_name}/versions/latest:access"
    )
    req = urllib.request.Request(url, headers={"Authorization": f"Bearer {access_token}"})
    with urllib.request.urlopen(req, timeout=30) as response:
        secret_data = json.loads(response.read().decode("utf-8"))
        payload = base64.b64decode(secret_data["payload"]["data"]).decode("utf-8")
        logger.debug(f"Successfully fetched secret: {secret_name}")
        return json.loads(payload)


def configure_runner_dirs() -> None:
    """Create and bind-mount the runner work directory onto a stateful path.

    Many GCE images have a stateful partition at `/mnt/stateful_partition` that
    persists across reboots. We place the runner workdir there and bind-mount
    it to `/var/lib/github` for the container to use as `_work`.
    """
    # Ensure destination exists with permissive mode (runner container writes here)
    subprocess.check_call(
        [
            "install",
            "--directory",
            "--owner",
            "root",
            "--group",
            "root",
            "--mode",
            "0777",
            "--verbose",
            "/mnt/stateful_partition/var/lib/github",
        ]
    )

    # Bind-mount stateful path to the expected runtime location
    subprocess.check_call(
        [
            "mount",
            "--bind",
            "/mnt/stateful_partition/var/lib/github",
            "/var/lib/github",
            "-o",
            "rw,nodev,relatime",
        ]
    )


def main() -> None:
    logger.info("Starting GitHub Actions runner setup...")
    configure_runner_dirs()

    project_id = get_project_id()
    region = get_region()

    logger.info("Configuring Docker registry mirrors...")
    try:
        virtual_repo = configure_docker(project_id=project_id, region=region)
    except Exception as e:
        logger.error("Error configuring Docker registry mirrors", exc_info=e)
        sys.exit(1)

    # Allow non-root processes (e.g. the runner container) to talk to Docker
    subprocess.check_call(["chmod", "a=rw", "/var/run/docker.sock"])

    logger.info("Reloading docker configuration...")
    try:
        subprocess.run(["systemctl", "reload", "docker"], check=True)
        logger.info("Docker configuration reloaded successfully")
    except Exception as e:
        logger.error("Error reloading Docker", exc_info=e)
        sys.exit(1)

    logger.info("Runner setup complete, creating systemd unit and starting runner...")
    try:
        write_systemd_unit(virtual_repo=virtual_repo)
        subprocess.check_call(["systemctl", "daemon-reload"])
        subprocess.check_call(["systemctl", "enable", "--now", "gha-runner.service"])
        logger.info("gha-runner.service enabled and started")
    except Exception as e:
        logger.error("Failed to start gha-runner via systemd", exc_info=e)
        sys.exit(1)


def configure_docker(region: str, project_id: str) -> str:
    """Configure Docker daemon with registry mirror and recommended settings.

    - Sets up `docker-credential-gcr` for Artifact Registry and GCR.
    - Adds an Artifact Registry virtual repository mirror for faster pulls.
    - Enables IPv6 in the Docker daemon (matching recommended defaults).

    Returns the URL of the virtual repository used as a mirror.
    """
    # /root is read-only, so use /tmp for the user docker config
    os.environ["DOCKER_CONFIG"] = "/tmp/.docker/"
    Path(os.environ["DOCKER_CONFIG"]).mkdir(parents=True, exist_ok=True)

    # Ensure docker-credential-gcr is installed and configure Docker to use it for authentication
    subprocess.check_call(
        ["docker-credential-gcr", "configure-docker", "--registries", f"gcr.io,{region}-docker.pkg.dev"])

    docker_dir = Path("/etc/docker")
    daemon_json_path = docker_dir / "daemon.json"

    # Create Docker directory if it doesn't exist
    try:
        docker_dir.mkdir(parents=True, exist_ok=True)
        logger.debug(f"Created Docker directory: {docker_dir}")
    except PermissionError:
        logger.exception("Permission denied creating /etc/docker directory")
        raise

    # Load existing configuration or create new one
    if daemon_json_path.exists():
        with daemon_json_path.open("r") as f:
            daemon_config = json.load(f)
        logger.info("Loaded existing daemon.json configuration")
    else:
        logger.info("Creating new daemon.json configuration...")
        daemon_config = {}

    # Add or update registry mirrors: use a region-local Artifact Registry virtual repo
    virtual_repo = f"{region}-docker.pkg.dev/{project_id}/virtual"
    daemon_config.setdefault("registry-mirrors", []).append(virtual_repo)

    # Update other recommended settings
    daemon_config["ipv6"] = True

    # Write the updated configuration
    with daemon_json_path.open("w") as f:
        json.dump(daemon_config, f, indent=2)

    logger.info(f"Successfully configured Docker registry mirror: {virtual_repo}")

    return virtual_repo


def write_systemd_unit(virtual_repo: str) -> Path:
    """Create a systemd unit to manage the runner container and VM lifecycle.

    The unit:
    - Starts a Docker container running the `actions/runner` image with JIT config.
    - Uses Google Cloud Logging (gcplogs) for container logs.
    - Powers off the VM once the container exits (i.e., after the job completes).
    """
    unit_path = Path("/etc/systemd/system/gha-runner.service")
    jit = get_jitconfig()

    image = f"{virtual_repo}/actions/actions-runner:latest"
    docker_run_line = (
        "/usr/bin/docker run "
        "--name gha-runner "
        "--log-driver=gcplogs "
        "--log-opt mode=non-blocking "
        "--log-opt max-buffer-size=4m "
        "--env DOCKER_BUILDKIT=1 "
        "--volume /var/run/docker.sock:/var/run/docker.sock "
        "--volume /var/lib/github:/runner/_work "
        f"{image} ./run.sh --jitconfig {jit}"
    )

    unit_contents = f"""
[Unit]
Description=GitHub Actions Runner (container)
After=docker.service
Requires=docker.service

[Service]
Type=exec
Environment="DOCKER_CONFIG=/tmp/.docker"

# Start the container detached
ExecStart={docker_run_line}

# Power off when it’s done
ExecStopPost=/usr/bin/systemctl poweroff

# Don’t restart the unit; the container exit ends the VM
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target
""".lstrip()

    unit_path.write_text(unit_contents)
    logger.info(f"Wrote systemd unit: {unit_path}")
    return unit_path


if __name__ == "__main__":
    # Configure logging with GCP JSON formatter
    logging.basicConfig(
        level=logging.INFO,
        handlers=[
            logging.StreamHandler(sys.stdout)
        ]
    )

    # Set GCP JSON formatter for all handlers
    for handler in logging.getLogger().handlers:
        handler.setFormatter(GCPJSONFormatter())

    main()
