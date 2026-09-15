# syntax=docker/dockerfile:1
FROM rust:1.85-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked -p agentsync-cli -p agentsync-server && \
    cargo build --release --locked -p agentsync-crypto --example showcase_identity && \
    mkdir /out && \
    cp target/release/agentsync target/release/agentsync-server /out/ && \
    cp target/release/examples/showcase_identity /out/agentsync-showcase-identity

FROM debian:bookworm-slim AS relay
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/* && \
    useradd --uid 10001 --create-home agentsync && mkdir /data && chown 10001:10001 /data
COPY --from=build /out/agentsync-server /usr/local/bin/
USER 10001:10001
ENTRYPOINT ["agentsync-server"]
CMD ["--data-dir", "/data/relay"]

FROM relay AS showcase
USER root
RUN apt-get update && apt-get install -y --no-install-recommends python3 git && \
    rm -rf /var/lib/apt/lists/*
COPY --from=build /out/agentsync /out/agentsync-showcase-identity /usr/local/bin/
COPY deploy/showcase /app/
COPY crates/agentsync-provider-codex/tests/fixtures/valid.jsonl /app/fixture.jsonl
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1
USER 10001:10001
EXPOSE 8788
ENTRYPOINT ["python3", "/app/server.py"]
