FROM rust:1.99-bookworm AS base
WORKDIR /app

# Used through `just`, with the source mounted from the host.
FROM base AS tools
RUN rustup component add rustfmt clippy \
    && cargo install diesel_cli --version 2.3.* --locked --no-default-features --features postgres

FROM base AS build
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked \
    && cp target/release/runmap /usr/local/bin/runmap

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system runmap
COPY --from=build /usr/local/bin/runmap /usr/local/bin/runmap
USER runmap
ENTRYPOINT ["runmap"]
