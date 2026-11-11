FROM rust:1.99-bookworm AS base
WORKDIR /app

# Used through `just`, with the source mounted from the host.
FROM base AS tools
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    rustup component add rustfmt clippy \
    && cargo install diesel_cli --version 2.3.* --locked --no-default-features --features postgres

FROM base AS build
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked \
    && cp target/release/firn /usr/local/bin/firn

# Newer than the build's Debian, which is fine: its C library reads older binaries.
FROM debian:trixie-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    # Fixed IDs, so that the files in an existing volume stay readable, and so
    # that docker-compose.yaml can name them.
    && groupadd --system --gid 999 firn \
    && useradd --system --uid 999 --gid 999 firn \
    && mkdir /data && chown firn:firn /data
COPY --from=build /usr/local/bin/firn /usr/local/bin/firn
USER 999:999
# Original files and photos. A new volume takes over the owner from here.
VOLUME /data
ENTRYPOINT ["firn"]
