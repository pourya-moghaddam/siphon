# ==========================================
# Stage 1: Build Siphon binaries
# ==========================================
FROM rust:1.82-slim-bookworm AS builder

WORKDIR /usr/src/siphon
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo build --release -p siphond -p siphon

# ==========================================
# Stage 2: Minimal Runtime Container
# ==========================================
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy compiled binaries
COPY --from=builder /usr/src/siphon/target/release/siphond /usr/local/bin/siphond
COPY --from=builder /usr/src/siphon/target/release/siphon /usr/local/bin/siphon

# Setup persistent volumes
RUN mkdir -p /data /downloads
VOLUME ["/data", "/downloads"]

ENV RUST_LOG=info
EXPOSE 9898

# Default run daemon listening on TCP for remote container attachment
ENTRYPOINT ["siphond"]
CMD ["--db", "/data/siphon.db", "--download-dir", "/downloads", "--socket", "/data/siphon.sock", "--listen", "0.0.0.0:9898"]
