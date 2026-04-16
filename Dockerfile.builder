FROM debian:bookworm-slim

# Host dependencies Buildroot needs to compile a full from-source system.
# Reference: https://buildroot.org/downloads/manual/manual.html#requirement
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        bison flex \
        libncurses-dev libssl-dev libelf-dev \
        perl python3 python-is-python3 \
        wget cpio rsync file unzip bzip2 xz-utils lzop \
        patch zip bc which \
        git ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

# Rust toolchain for cross-compiling bunzo's own userland (M2+).
# Installed under /opt/rust so it's owned by root and readable by any uid
# the build container runs as (build-docker.sh --user $HOST_UID:$HOST_GID).
# CARGO_HOME is redirected to /bunzo-cargo (a Docker named volume) at run
# time so the package cache survives across container runs.
ENV RUSTUP_HOME=/opt/rust/rustup \
    CARGO_HOME_DEFAULT=/opt/rust/cargo \
    PATH=/opt/rust/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | CARGO_HOME=/opt/rust/cargo sh -s -- \
            -y --no-modify-path --profile minimal \
            --default-toolchain stable \
            --target aarch64-unknown-linux-musl \
    && chmod -R a+rX /opt/rust

WORKDIR /src
