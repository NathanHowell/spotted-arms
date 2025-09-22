# Multi-stage Dockerfile for spotted-arms
# Stage 1: Build the Rust application
FROM rust:1.90-slim AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    ca-certificates \
    build-essential \
    && rm -rf /var/lib/apt/lists/*

# Set the working directory
WORKDIR /app

# Copy dependency files first for better caching
COPY Cargo.toml Cargo.lock ./

# Create a dummy main.rs to build dependencies
RUN mkdir -p src/bin && echo "fn main() {println!(\"hello\");}" > src/bin/spotted-arms.rs && \
    echo "// dummy lib" > src/lib.rs

# Build dependencies
RUN cargo build --release --bin spotted-arms
RUN rm -rf src

# Copy the actual source code
COPY src ./src
COPY .cargo ./.cargo

# Build the application in release mode for x86_64
RUN cargo build --release --bin spotted-arms

# Stage 2: Create the runtime image using distroless
FROM gcr.io/distroless/cc-debian12:latest

# Copy the binary from the builder stage
COPY --from=builder /app/target/release/spotted-arms /usr/local/bin/spotted-arms

# Create a non-root user for security
USER 65534:65534

# Expose the default port (can be overridden via PORT env var)
EXPOSE 3000

# Set the entrypoint
ENTRYPOINT ["/usr/local/bin/spotted-arms"]