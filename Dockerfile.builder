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
        git ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
