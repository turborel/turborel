# Container image for the turborel server.
#
# Contains turboreld (the server) and turborel (the command-line client).
#
#   docker build -t turborel .
#   docker buildx build --platform linux/amd64,linux/arm64 -t turborel .

# Build stage. The Rust version must match rust-toolchain.toml; the check
# below fails the build if it does not.
FROM rust:1.99.1-slim-trixie@sha256:f6c4a35d21dbb3c89d7a8d056c193e94fc09037f2b58dd858b09c39a47dd02d7 AS build

# Use the compiler already in the image. Otherwise rustup reads
# rust-toolchain.toml and downloads clippy, rustfmt and llvm-tools, which a
# release build does not need.
ENV RUSTUP_TOOLCHAIN=${RUST_VERSION}

# The CPU the compiler may assume, passed to rustc as -C target-cpu.
# Empty, the default, assumes the architecture's baseline alone, and the
# binaries run on every CPU of that architecture. A level such as
# x86-64-v3 lets the compiler emit that level's instructions (AVX2, BMI2
# and FMA among them), and the binaries then die of SIGILL, illegal
# instruction, on any CPU without them. `rustc --print target-cpus` lists
# the names the compiler accepts.
ARG TARGET_CPU=""
# Set by BuildKit to the architecture being built, for example arm64.
ARG TARGETARCH

WORKDIR /src
COPY . .

RUN channel="$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)"; \
    if [ "$channel" != "$RUST_VERSION" ]; then \
        echo "rust-toolchain.toml pins Rust $channel, the build image holds $RUST_VERSION" >&2; \
        exit 1; \
    fi

# The cache mounts keep downloaded crates and compiled objects between
# builds, with one target directory per architecture. Cache mounts are not
# part of the image, so the results are copied to /out.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=turborel-target-${TARGETARCH},target=/src/target,sharing=locked \
    if [ -n "$TARGET_CPU" ]; then export RUSTFLAGS="-C target-cpu=$TARGET_CPU"; fi; \
    cargo build --release --locked -p turboreld -p turborel-cli \
    && install -D -t /out/usr/local/bin target/release/turboreld target/release/turborel \
    && install -D -m 0644 -t /out/usr/share/doc/turborel LICENSE \
    && install -d -m 0700 /out/var/lib/turborel

# Runtime stage. Distroless: glibc, libgcc and CA certificates, with no
# shell and no package manager.
FROM gcr.io/distroless/cc-debian13:nonroot@sha256:cb87acc574abe719b6e5c547384defa2f7b1f24af9605d3f618cf852495d8be2

LABEL org.opencontainers.image.title="turborel" \
      org.opencontainers.image.description="The turborel relational database: turboreld, its server, and turborel, its command-line client." \
      org.opencontainers.image.source="https://github.com/turborel/turborel" \
      org.opencontainers.image.licenses="BSD-2-Clause-Patent"

COPY --from=build /out/usr/ /usr/
# Data directory, owned by the user the server runs as.
COPY --from=build --chown=65532:65532 --chmod=0700 /out/var/lib/turborel/ /var/lib/turborel/

# The distroless "nonroot" user. Given as a number so that Kubernetes can
# verify the container does not run as root.
USER 65532:65532
WORKDIR /var/lib/turborel

# Keep the database on a volume, not in the container's writable layer.
VOLUME /var/lib/turborel

# 7422: client connections. 7423: replication between nodes.
EXPOSE 7422 7423

# turboreld runs as PID 1 and handles SIGTERM itself, so no init process or
# entrypoint script is needed. It listens on 127.0.0.1 by default, which is
# unreachable from outside the container, so listen on all interfaces.
ENTRYPOINT ["/usr/local/bin/turboreld"]
CMD ["--dir", "/var/lib/turborel/data", "--set", "SERVER.HOST=0.0.0.0"]
