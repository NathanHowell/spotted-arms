#!/usr/bin/env python3
"""
GitHub Actions runner startup script
Python version of cloud-init.sh
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

logger = logging.getLogger(__name__)


class GCPJSONFormatter(logging.Formatter):
    """JSON formatter optimized for GCP Cloud Logging"""

    def format(self, record):
        # Build the log entry following GCP Cloud Logging structure
        log_entry = {
            "severity": record.levelname or "DEFAULT",
            "message": record.getMessage(),
            "timestamp": {
                "seconds": int(record.created),
                "nanos": int((record.created % 1) * 1e9)
            },
            "sourceLocation": {
                "file": record.pathname,
                "line": str(record.lineno),
                "function": record.funcName
            },
            "labels": {
                "component": "github-actions-runner",
                "module": record.module
            }
        }

        # Add exception information if present
        if record.exc_info:
            log_entry["exception"] = self.formatException(record.exc_info)

        # Add any extra fields from the log record
        if hasattr(record, "extra_fields"):
            log_entry.update(record.extra_fields)

        return json.dumps(log_entry)


def get_project_id():
    """Get project ID from metadata server"""
    req = urllib.request.Request(
        "http://metadata.google.internal/computeMetadata/v1/project/project-id",
        headers={"Metadata-Flavor": "Google"}
    )
    response: HTTPResponse
    with urllib.request.urlopen(req, timeout=10) as response:
        project_id = response.read().decode("utf-8").strip()
        logger.debug(f"Retrieved project ID: {project_id}")
        return project_id


def get_region():
    """Get project ID from metadata server"""
    req = urllib.request.Request(
        "http://metadata.google.internal/computeMetadata/v1/instance/zone",
        headers={"Metadata-Flavor": "Google"}
    )
    response: HTTPResponse
    with urllib.request.urlopen(req, timeout=10) as response:
        zone = response.read().decode("utf-8").strip()
        logger.debug(f"Retrieved : {zone}")
        # Zone format: projects/12345/zones/us-central1-a
        return zone.partition("/zones/")[-1].rsplit("-", 1)[0]


def get_jitconfig():
    """Get access token from metadata server"""
    req = urllib.request.Request(
        "http://metadata.google.internal/computeMetadata/v1/instance/attributes/JIT_CONFIG",
        headers={"Metadata-Flavor": "Google"}
    )
    response: HTTPResponse
    with urllib.request.urlopen(req, timeout=10) as response:
        return response.read().decode("utf-8")


def fetch_secret(secret_name, project_id, access_token):
    """Fetch secret from Google Secret Manager"""
    url = f"https://secretmanager.googleapis.com/v1/projects/{project_id}/secrets/{secret_name}/versions/latest:access"
    req = urllib.request.Request(
        url,
        headers={"Authorization": f"Bearer {access_token}"}
    )
    with urllib.request.urlopen(req, timeout=30) as response:
        secret_data = json.loads(response.read().decode("utf-8"))
        payload = base64.b64decode(secret_data["payload"]["data"]).decode("utf-8")
        logger.debug(f"Successfully fetched secret: {secret_name}")
        return json.loads(payload)


def configure_runner_dirs() -> None:
    subprocess.check_call(
        ["install", "--directory", "--owner", "root", "--group", "root", "--mode", "0777", "--verbose",
         "/mnt/stateful_partition/var/lib/github"])

    subprocess.check_call(
        ["mount", "--bind", "/mnt/stateful_partition/var/lib/github", "/var/lib/github", "-o", "rw,nodev,relatime"]
    )


def main():
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

    logger.info("Opening up docker for the world, I am so sorry")
    subprocess.check_call(["chmod", "a=rw", "/var/run/docker.sock"])

    logger.info("Reloading docker configuration...")
    try:
        subprocess.run(["systemctl", "reload", "docker"], check=True)
        logger.info("Docker configuration reloaded successfully")
    except Exception as e:
        logger.error(f"Error reloading Docker", exc_info=e)
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
    """Configure Docker daemon with registry mirrors and recommended settings"""
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

    # Add or update registry mirrors
    virtual_repo = f"{region}-docker.pkg.dev/{project_id}/virtual"
    daemon_config.setdefault("registry-mirrors", []).append(virtual_repo)

    # Update other recommended settings
    daemon_config["ipv6"] = True

    # Write the updated configuration
    with daemon_json_path.open("w") as f:
        json.dump(daemon_config, f, indent=2)

    logger.info(f"Successfully configured Docker registry mirrors: {virtual_repo}")

    return virtual_repo


def write_systemd_unit(virtual_repo: str) -> Path:
    """Create a systemd unit to manage the GitHub Actions runner container.

    Keeps existing docker parameters and adds logging + lifecycle handling.
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
