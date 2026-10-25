# Turmite server image: one binary on distroless, as a non-root user.
#   docker build -t turmite .
#   docker buildx build --platform linux/amd64,linux/arm64 -t turmite .
# The builder runs on the build machine and cross-compiles to the target, so a
# multi-arch build needs no emulation. Migrations, templates, assets and the
# deposit contract (contracts/out-min) are compiled into the binary.

FROM --platform=$BUILDPLATFORM rust:1.99.1-slim-trixie AS builder
ARG TARGETARCH
WORKDIR /src
# aws-lc-rs (EVM) needs a C toolchain, cmake and perl.
RUN set -eux; \
    case "$TARGETARCH" in \
      amd64) triple=x86_64-unknown-linux-gnu; gnu=x86-64-linux-gnu; libc=amd64; cc=x86_64-linux-gnu ;; \
      arm64) triple=aarch64-unknown-linux-gnu; gnu=aarch64-linux-gnu; libc=arm64; cc=aarch64-linux-gnu ;; \
      *) echo "unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    pkgs="cmake perl pkg-config"; \
    if [ "$(dpkg --print-architecture)" = "$libc" ]; then \
      pkgs="$pkgs gcc g++ libc6-dev"; \
    else \
      pkgs="$pkgs gcc-$gnu g++-$gnu libc6-dev-$libc-cross"; \
      upper=$(echo "$triple" | tr 'a-z-' 'A-Z_'); \
      { echo "export CARGO_TARGET_${upper}_LINKER=$cc-gcc"; \
        echo "export CC_$(echo "$triple" | tr - _)=$cc-gcc"; \
        echo "export CXX_$(echo "$triple" | tr - _)=$cc-g++"; \
        echo "export AR_$(echo "$triple" | tr - _)=$cc-ar"; } > /cross.env; \
    fi; \
    touch /cross.env; \
    echo "$triple" > /triple; \
    apt-get update; \
    apt-get install -y --no-install-recommends $pkgs; \
    rm -rf /var/lib/apt/lists/*
COPY rust-toolchain.toml ./
RUN rustup target add "$(cat /triple)"
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY contracts/out-min contracts/out-min
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,id=turmite-target-$TARGETARCH,sharing=locked \
    set -eux; \
    . /cross.env; \
    triple=$(cat /triple); \
    cargo build --release --locked -p turmite --features all-chains --target "$triple"; \
    mkdir -p /out; \
    cp "target/$triple/release/turmite" /out/turmite

# distroless/cc: glibc, libgcc, CA certificates and tzdata. No shell and no
# package manager, so a compromised process has few tools. Pinned by digest.
FROM gcr.io/distroless/cc-debian13:nonroot@sha256:83b9b50586253821cf63ffcec87d3d94265a093ad30da232039af962dd8f4edf
ARG VERSION=dev
LABEL org.opencontainers.image.title="Turmite" \
      org.opencontainers.image.description="Self-hosted crypto payment gateway" \
      org.opencontainers.image.source="https://github.com/loruorg/turmite" \
      org.opencontainers.image.url="https://turmite.com" \
      org.opencontainers.image.documentation="https://turmite.com/docs" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.version="$VERSION"
COPY --from=builder /out/turmite /usr/local/bin/turmite
# `turmite init` writes into the current directory: mount the deploy directory here.
WORKDIR /data
# 65532 is the distroless `nonroot` user. The worker reads secrets/ as this user.
USER 65532:65532
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=60s --retries=3 CMD ["/usr/local/bin/turmite", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/turmite"]
CMD ["serve"]
