FROM rust:slim AS build
WORKDIR /app

RUN apt-get update && apt-get install -y ca-certificates curl gnupg python3 python3-dev pkg-config libssl-dev libsqlite3-dev && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - && \
    apt-get install -y nodejs && \
    rm -rf /var/lib/apt/lists/* && \
    corepack enable && \
    corepack prepare pnpm@11.2.2 --activate

RUN set -ex; \
    cargo install diesel_cli --no-default-features --features sqlite-bundled --force; \
    cp -r "${CARGO_HOME:-$HOME/.cargo}/bin/diesel" /usr/local/bin/diesel;

RUN --mount=type=bind,source=/static,target=static,rw \
    --mount=type=bind,source=/web,target=web \
    --mount=type=bind,source=/crates,target=crates \
    --mount=type=bind,source=/migrations,target=migrations \
    --mount=type=bind,source=/Cargo.toml,target=Cargo.toml \
    --mount=type=bind,source=/Cargo.lock,target=Cargo.lock \
    --mount=type=bind,source=/package.json,target=package.json \
    --mount=type=bind,source=/pnpm-lock.yaml,target=pnpm-lock.yaml \
    --mount=type=bind,source=/pnpm-workspace.yaml,target=pnpm-workspace.yaml \
    --mount=type=bind,source=/rollup.config.mjs,target=rollup.config.mjs \
    --mount=type=cache,target=/app/target/ \
    --mount=type=cache,target=/usr/local/cargo/registry/ \
    <<EOF
set -e
export HUSKY=0
cargo build --locked --release
pnpm install --frozen-lockfile
pnpm run build:prod
cp ./target/release/server /usr/local/bin/server
cp ./target/release/collector /usr/local/bin/collector
cp ./target/release/mcp /usr/local/bin/mcp
cp ./target/release/migrate /usr/local/bin/migrate
cp -r ./static /usr/local/bin/static
cp -r ./web /usr/local/bin/web
EOF


FROM debian:stable-slim AS final
RUN apt-get update && apt-get install -y git python3 python3-dev libssl-dev libsqlite3-0 && rm -rf /var/lib/apt/lists/*

ARG UID=10001
RUN adduser \
    --disabled-password \
    --gecos "" \
    --home "/nonexistent" \
    --shell "/sbin/nologin" \
    --no-create-home \
    --uid "${UID}" \
    appuser
RUN mkdir /app

COPY --from=build /usr/local/bin/diesel /usr/local/bin/diesel
COPY --from=build /usr/local/bin/server /usr/local/bin/oghserver
COPY --from=build /usr/local/bin/collector /usr/local/bin/oghcollector
COPY --from=build /usr/local/bin/mcp /usr/local/bin/oghmcp
COPY --from=build /usr/local/bin/migrate /usr/local/bin/oghmigrate
COPY --from=build /usr/local/bin/static /app/static/
COPY --from=build /usr/local/bin/web/templates /app/web/templates
COPY ./files/pip_names.txt /app/files/pip_names.txt
COPY ./docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh

RUN set -ex; \
    mkdir /app/data; \
    chown -R appuser:appuser /app; \
    chmod 755 /usr/local/bin/oghserver /usr/local/bin/oghcollector /usr/local/bin/oghmcp /usr/local/bin/oghmigrate /usr/local/bin/diesel /usr/local/bin/docker-entrypoint.sh;

USER appuser
EXPOSE 8080

WORKDIR /app
ENTRYPOINT ["docker-entrypoint.sh"]
CMD ["oghserver"]
