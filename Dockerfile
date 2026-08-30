# syntax=docker/dockerfile:1

# =============================================================================
# Build stage
# =============================================================================
FROM rust:1-slim-trixie AS build

WORKDIR /app

# Build deps only (not shipped to final)
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    gnupg \
    g++ \
    pkg-config \
    libssl-dev \
    libsqlite3-dev \
    python3 \
    python3-dev \
    libpython3-dev \
    && rm -rf /var/lib/apt/lists/*

ENV PYO3_PYTHON=python3

# Node 22 + pnpm (aligned with package.json: pnpm@11.10.0)
RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/* \
    && corepack enable \
    && corepack prepare pnpm@11.10.0 --activate

ENV CARGO_TERM_COLOR=always \
    CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse \
    HUSKY=0

# diesel_cli
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    set -eux; \
    cargo install diesel_cli \
    --no-default-features \
    --features sqlite-bundled \
    --force; \
    install -m 755 /usr/local/cargo/bin/diesel /usr/local/bin/diesel

# Bins + assets
RUN --mount=type=bind,source=Cargo.toml,target=Cargo.toml \
    --mount=type=bind,source=Cargo.lock,target=Cargo.lock \
    --mount=type=bind,source=crates,target=crates \
    --mount=type=bind,source=migrations,target=migrations \
    --mount=type=bind,source=package.json,target=package.json \
    --mount=type=bind,source=pnpm-lock.yaml,target=pnpm-lock.yaml \
    --mount=type=bind,source=pnpm-workspace.yaml,target=pnpm-workspace.yaml \
    --mount=type=bind,source=rollup.config.mjs,target=rollup.config.mjs \
    --mount=type=bind,source=web,target=web \
    --mount=type=bind,source=static,target=static,rw \
    --mount=type=cache,target=/app/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/root/.local/share/pnpm/store \
    <<'EOF'
set -eu pipefail

# 1) Build Release
cargo build --locked --release

# 2) Frontend prod
pnpm install --frozen-lockfile
pnpm run build:prod

# 3) Artifacts
mkdir -p /out/bin /out/static /out/templates
cp -a target/release/server    /out/bin/server
cp -a target/release/collector /out/bin/collector
cp -a target/release/mcp       /out/bin/mcp
cp -a target/release/migrate   /out/bin/migrate
cp -a static/.                 /out/static/
cp -a web/templates/.          /out/templates/
EOF

# =============================================================================
# Runtime
# =============================================================================
FROM debian:trixie-slim AS final

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    git \
    python3 \
    libpython3.13 \
    libssl3 \
    libsqlite3-0 \
    && rm -rf /var/lib/apt/lists/*

ARG UID=10001
RUN useradd \
    --uid "${UID}" \
    --comment "" \
    --home-dir "/nonexistent" \
    --shell "/sbin/nologin" \
    --no-create-home \
    appuser \
    && mkdir -p /app/data /app/static /app/web/templates /app/files

# diesel CLI
COPY --from=build /usr/local/bin/diesel /usr/local/bin/diesel

# Binaries
COPY --from=build /out/bin/server    /usr/local/bin/oghserver
COPY --from=build /out/bin/collector /usr/local/bin/oghcollector
COPY --from=build /out/bin/mcp       /usr/local/bin/oghmcp
COPY --from=build /out/bin/migrate   /usr/local/bin/oghmigrate

# Frontend + templates
COPY --from=build /out/static    /app/static
COPY --from=build /out/templates /app/web/templates

COPY files/pip_names.txt /app/files/pip_names.txt
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh

RUN chown -R appuser:appuser /app \
    && chmod 755 \
    /usr/local/bin/diesel \
    /usr/local/bin/oghserver \
    /usr/local/bin/oghcollector \
    /usr/local/bin/oghmcp \
    /usr/local/bin/oghmigrate \
    /usr/local/bin/docker-entrypoint.sh

USER appuser
WORKDIR /app
EXPOSE 8080 8081
ENTRYPOINT ["docker-entrypoint.sh"]
CMD ["oghserver"]
