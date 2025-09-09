# Spotted Arms - GitHub Actions Runner Provisioner

Always reference these instructions first and fallback to search or bash commands only when you encounter unexpected information that does not match the info here.

Spotted Arms is a Rust service that listens for GitHub `workflow_job` webhooks and provisions ephemeral self-hosted runners on Google Compute Engine using region instance templates. When jobs complete, the service deletes the instances.

## Working Effectively

### Prerequisites and Dependencies
- Rust 1.89.0+ (toolchain is already installed in the environment)
- Docker (for containerized deployments)
- Google Cloud credentials and project setup (for actual deployment)

### Bootstrap, Build, and Test Commands
- **NEVER CANCEL** any build or test commands. Build and test processes can take several minutes.
- Always run these commands in sequence for a fresh clone:

```bash
# Check and validate project (quick validation)
cargo check  # Takes ~1-2 minutes. NEVER CANCEL. Set timeout to 10+ minutes.

# Format code (required before committing)
cargo fmt

# Build release version (required for deployment)
cargo build --release --bin spotted-arms  # Takes ~4 minutes. NEVER CANCEL. Set timeout to 10+ minutes.

# Run tests (validates functionality)
cargo test  # Takes ~2 minutes. NEVER CANCEL. Set timeout to 10+ minutes.
```

### Run the Application
- **Application requires specific environment variables** and will error gracefully if missing
- Test without required environment (shows expected behavior):
```bash
cargo run --bin spotted-arms  # Will fail with GCP metadata error - this is expected
```

- Get help and see all configuration options:
```bash
cargo run --bin spotted-arms -- --help
```

- Required environment variables for actual operation:
  - `GITHUB_CREDENTIALS` - JSON with GitHub token and webhook secret
  - `INSTANCE_TEMPLATE` - GCE region instance template name
  - `GOOGLE_CLOUD_PROJECT` - Google Cloud project ID
  - `GOOGLE_CLOUD_ZONE` - Google Cloud zone (must be us-central1-*)

### Docker Support
- Docker build is supported but **fails in some environments due to SSL certificate issues**
- Fixed .dockerignore to include Cargo.lock (was missing)
- If Docker build fails with SSL errors, use local Rust builds instead
- Docker build command: `docker build -t spotted-arms:test .`

## Validation

### Code Quality and Linting
- **Always run before committing changes:**
```bash
cargo fmt      # Fixes code formatting
cargo clippy   # Shows linting warnings - there are known warnings in the codebase
```

- Known clippy warnings exist in the codebase (unused extern crates, style issues)
- Do not treat clippy warnings as build failures unless they're new errors you introduced

### Testing Strategy
- **Always run complete test suite after making changes:**
```bash
cargo test  # Runs all unit tests and integration tests (16 tests, 1 ignored)
```
- Tests include unit tests, integration tests, and doc tests
- One test is ignored (`test_create_instance`) - this is expected
- Tests use mock implementations for GCP and GitHub APIs

### Manual Validation Scenarios
- **Always test the help command:** `cargo run --bin spotted-arms -- --help`
- **Verify graceful error handling:** Run without environment variables to confirm proper error messages
- **Validate configuration:** Check that all environment variable options are documented in help output

## Build Times and Timeouts

### Command Timing (with safety margins)
- `cargo check`: ~1-2 minutes → **Use 10+ minute timeout**
- `cargo build --release`: ~4 minutes → **Use 10+ minute timeout**  
- `cargo test`: ~2 minutes → **Use 10+ minute timeout**
- `cargo fmt`: <1 second → Use default timeout
- `cargo clippy`: ~3-4 minutes → **Use 10+ minute timeout**

### **CRITICAL: NEVER CANCEL BUILD COMMANDS**
- Rust compilation can take several minutes, especially for release builds
- Always set appropriate timeouts (10+ minutes minimum)
- Build artifacts are stored in `target/` directory (~3GB when built)

## Repository Structure

### Key Source Files (15 total .rs files)
```
src/
├── bin/spotted-arms.rs    # Main application entry point
├── lib.rs                 # Library root
├── server.rs             # Axum HTTP server and routing
├── webhook.rs            # GitHub webhook handler
├── instance.rs           # GCE instance management
├── compute.rs            # Google Compute Engine API client
├── github.rs             # GitHub API client  
├── metadata.rs           # GCP metadata utilities
├── telemetry.rs          # OpenTelemetry and logging setup
└── utils.rs              # Utility functions
```

### Test Files
```
tests/
├── handler_flow.rs           # Integration tests for webhook handling
├── webhook_integration.rs    # Webhook payload parsing tests
└── fixtures/                # Test webhook payloads
    ├── completed-payload.json
    ├── in-progress-payload.json
    └── queued-payload.json
```

### Configuration Files
- `Cargo.toml` - Rust project configuration and dependencies
- `Dockerfile` - Multi-stage Docker build (has SSL issues in some environments)
- `.cargo/config.toml` - Rust compiler flags for unused dependencies
- `.dockerignore` - Docker ignore patterns (Cargo.lock is included)

## Common Tasks

### Development Workflow
1. Make code changes
2. Run `cargo fmt` to format code
3. Run `cargo check` to validate syntax
4. Run `cargo test` to ensure tests pass
5. Run `cargo clippy` to check for style issues (warnings are OK)
6. Build with `cargo build --release` if deploying

### Debugging Build Issues
- Check that Rust 1.89.0+ is installed: `rustc --version`
- Clean build cache if needed: `cargo clean` (removes ~3GB target/ directory)
- Rebuild dependencies: `cargo update`

### Environment Variables for Testing
- The application requires GCP and GitHub credentials for actual operation
- For testing code changes, you can run most commands without credentials
- Webhook endpoints are available at `/webhook`, `/ping`, and `/health_check`

### Common Endpoints (when running)
- `POST /webhook` - GitHub webhook receiver for workflow_job events
- `GET /ping` - Simple liveness probe (returns "pong")  
- `POST /health_check` - Returns JSON status and request headers

## Platform-Specific Notes
- Supports only `us-central1` region for GCE instances
- Requires specific job labels: `linux`, `self-hosted`, `ARM64`
- Uses deterministic zone selection within the region
- Structured JSON logging with OpenTelemetry support