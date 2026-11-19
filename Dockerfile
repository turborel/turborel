# turboreld and turborel on a distroless base.
#
#   docker build -t turborel .
#   docker buildx build --platform linux/amd64,linux/arm64 -t turborel .

FROM rust:1.99.1-slim-trixie@sha256:f6c4a35d21dbb3c89d7a8d056c193e94fc09037f2b58dd858b09c39a47dd02d7 AS build

# Keep rustup from reading rust-toolchain.toml and fetching its components.
ENV RUSTUP_TOOLCHAIN=${RUST_VERSION}

# -C target-cpu. Empty runs on every CPU of the architecture; x86-64-v3
# and the like die of SIGILL on older ones.
ARG TARGET_CPU=""
ARG TARGETARCH

WORKDIR /src
COPY . .

# The image's compiler and rust-toolchain.toml are one configuration item (RULES.md D174).
RUN channel="$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)"; \
    if [ "$channel" != "$RUST_VERSION" ]; then \
        echo "rust-toolchain.toml pins Rust $channel, the build image holds $RUST_VERSION" >&2; \
        exit 1; \
    fi

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=turborel-target-${TARGETARCH},target=/src/target,sharing=locked \
    if [ -n "$TARGET_CPU" ]; then export RUSTFLAGS="-C target-cpu=$TARGET_CPU"; fi; \
    cargo build --release --locked -p turboreld -p turborel-cli \
    && install -D -t /out/usr/local/bin target/release/turboreld target/release/turborel \
    && install -D -m 0644 -t /out/usr/share/doc/turborel LICENSE \
    && install -d -m 0700 /out/var/lib/turborel

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:cb87acc574abe719b6e5c547384defa2f7b1f24af9605d3f618cf852495d8be2

LABEL org.opencontainers.image.title="turborel" \
      org.opencontainers.image.description="The turborel relational database: turboreld, its server, and turborel, its command-line client." \
      org.opencontainers.image.source="https://github.com/turborel/turborel" \
      org.opencontainers.image.licenses="BSD-2-Clause-Patent"

COPY --from=build /out/usr/ /usr/
COPY --from=build --chown=65532:65532 --chmod=0700 /out/var/lib/turborel/ /var/lib/turborel/

# Numeric so that Kubernetes' runAsNonRoot can check it.
USER 65532:65532
WORKDIR /var/lib/turborel
VOLUME /var/lib/turborel

# 7422 clients, 7423 replication.
EXPOSE 7422 7423

# 127.0.0.1, the default, is unreachable from outside the container.
ENTRYPOINT ["/usr/local/bin/turboreld"]
CMD ["--dir", "/var/lib/turborel/data", "--set", "SERVER.HOST=0.0.0.0"]
