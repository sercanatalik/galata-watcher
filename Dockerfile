# galata-watcher: the three binaries and the cereyan lane, in one image.
#
#   docker buildx build --platform linux/amd64,linux/arm64 -t galata-watcher .
#
# The binaries are built against glibc on Debian bookworm, the same release
# the runtime image is based on. (Not static musl: galata-desk-core's rustls
# uses aws-lc-rs, whose C build is the fragile part on musl.)

FROM rust:1.98.1-bookworm AS build
WORKDIR /src
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release --locked \
        -p galata-watcher-poll -p galata-watcher-api -p galata-notify \
 && mkdir /out \
 && cp target/release/galata-watcher-poll target/release/galata-watcher-api target/release/galata-notify /out/

FROM python:3.12-slim-bookworm
COPY --from=ghcr.io/astral-sh/uv:0.13.8 /uv /usr/local/bin/uv
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --uid 10001 --create-home watcher \
 && mkdir /data && chown watcher /data

WORKDIR /app/lane
COPY lane/pyproject.toml lane/uv.lock ./
RUN uv sync --frozen --no-dev
COPY lane/lane.toml ./
COPY lane/flows ./flows
COPY --from=build /out/ /usr/local/bin/

ENV CEREYAN_HOME=/data/cereyan \
    PATH=/app/lane/.venv/bin:$PATH
USER watcher
# The lane. The API service in compose.yaml runs the same image with its
# own command. cereyan's UI listens only inside the container.
CMD ["cereyan", "serve", ".", "--host", "127.0.0.1", "--port", "4200", "--no-open"]
