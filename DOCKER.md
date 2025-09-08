# Docker Configuration

This directory contains the Docker configuration for building and running spotted-arms in a distroless container.

## Building the Image

```bash
docker build -t spotted-arms:latest .
```

## Running the Container

```bash
# Basic run with default configuration
docker run -p 3000:3000 spotted-arms:latest

# With environment variables
docker run -p 3000:3000 \
  -e GITHUB_CREDENTIALS='{"token":"your-token","secret":"your-secret"}' \
  -e INSTANCE_TEMPLATE=your-template-name \
  -e GOOGLE_CLOUD_PROJECT=your-project \
  -e GOOGLE_CLOUD_ZONE=us-central1-f \
  spotted-arms:latest
```

## Container Features

- **Multi-stage build**: Optimized for size with separate build and runtime stages
- **Distroless runtime**: Minimal attack surface using Google's distroless base image
- **Non-root user**: Runs as user 65534 for security
- **x86_64 architecture**: Built specifically for x64 platforms
- **Optimized caching**: Dependencies are built separately for faster rebuilds

## Environment Variables

The container supports all environment variables documented in the main README:

- `PORT` - HTTP server port (default: 3000)
- `GITHUB_CREDENTIALS` - JSON with GitHub token and webhook secret
- `INSTANCE_TEMPLATE` - GCE region instance template name
- `GOOGLE_CLOUD_PROJECT` - Google Cloud project ID
- `GOOGLE_CLOUD_ZONE` - Google Cloud zone
- And others as documented in the main README

## Health Checks

The container exposes the following endpoints:
- `GET /ping` - Liveness probe
- `POST /health_check` - Detailed health check

## GitHub Actions Integration

An example GitHub Actions workflow for building and pushing the Docker image is provided in `.github/workflows/docker.yml.example`. This workflow:

- Builds the image for linux/amd64 platform
- Pushes to GitHub Container Registry
- Includes security scanning with Trivy
- Uses Docker layer caching for faster builds

To use it, rename the file to `docker.yml` and customize as needed.