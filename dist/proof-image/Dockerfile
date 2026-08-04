# syntax=docker/dockerfile:1.7

FROM --platform=linux/amd64 docker.io/library/rust@sha256:858f8882676b21c915fb5fab0261fc80d47c21f00faf9f610db89f966978eca6

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    apt-get update && \
    apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        docker.io \
        git \
        musl-tools \
        pkg-config

RUN rustup target add wasm32-unknown-unknown x86_64-unknown-linux-musl

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo install --locked --root /opt/spice/tools --version 0.21.14 trunk && \
    cargo install --locked --root /opt/spice/tools --version 27.1.0 cargo-mutants && \
    cargo install --locked --root /opt/spice/tools --version 0.2.126 wasm-bindgen-cli

COPY vendor /opt/spice/vendor
COPY cargo-config.toml /opt/spice/cargo-config.toml
COPY cargo-wrapper.sh /opt/spice/bin/cargo

RUN chmod 0755 /opt/spice/bin/cargo && \
    git config --system safe.directory '*' && \
    /opt/spice/bin/cargo --version && \
    /opt/spice/tools/bin/trunk --version && \
    /opt/spice/tools/bin/cargo-mutants mutants --version && \
    /opt/spice/tools/bin/wasm-bindgen --version && \
    rustup target list --installed

ENV CARGO_NET_OFFLINE=true \
    PATH=/opt/spice/bin:/opt/spice/tools/bin:/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    RUSTUP_TOOLCHAIN=1.97.1 \
    TMPDIR=/cargo-target/tmp \
    TRUNK_OFFLINE=true

WORKDIR /workspace
