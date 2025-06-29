FROM rust:slim AS build
WORKDIR /app

RUN apt-get update && apt-get install -y python3 python3-dev pkg-config libssl-dev npm && rm -rf /var/lib/apt/lists/*

RUN --mount=type=bind,source=/static,target=static,rw \
    --mount=type=bind,source=/web,target=web \
    --mount=type=bind,source=/crates,target=crates \
    --mount=type=bind,source=/Cargo.toml,target=Cargo.toml \
    --mount=type=bind,source=/Cargo.lock,target=Cargo.lock \
    --mount=type=bind,source=/package.json,target=package.json \
    --mount=type=bind,source=/rollup.config.mjs,target=rollup.config.mjs \
    --mount=type=cache,target=/app/target/ \
    --mount=type=cache,target=/usr/local/cargo/registry/ \
    <<EOF
set -e
cargo build --locked --release
npm install -y
npm run build:prod
cp ./target/release/server /usr/local/bin/server
cp ./target/release/collector /usr/local/bin/collector
cp -r ./static /usr/local/bin/static
cp -r ./web /usr/local/bin/web
EOF


FROM debian:stable-slim AS final
RUN apt-get update && apt-get install -y git python3 python3-dev libssl-dev && rm -rf /var/lib/apt/lists/*

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

COPY --from=build /usr/local/bin/server /usr/local/bin/oghserver
COPY --from=build /usr/local/bin/collector /usr/local/bin/oghcollector
COPY --from=build /usr/local/bin/static /app/static/
COPY --from=build /usr/local/bin/web/templates /app/web/templates

RUN set -ex; \
    mkdir /app/data; \
    chown -R appuser:appuser /app; \
    chmod 755 /usr/local/bin/oghserver /usr/local/bin/oghcollector;

USER appuser
EXPOSE 8080

WORKDIR /app
CMD ["oghserver"]
