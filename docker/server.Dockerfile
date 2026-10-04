# syntax=docker/dockerfile:1.7

# Build the web UI as a static SvelteKit bundle and the mmcp-server binary with cargo-chef dependency caching.
# Both are copied into a distroless runtime image.
# The server serves the bundle on its own origin from MMCP_WEBUI_DIR.
# Every stage that compiles runs on the build machine's architecture and cross-compiles the binary for the target platform through xx.
# No stage runs under emulation, so a build for any platform is fast on any host.
# The dependency graph still compiles C and links the system OpenSSL through webauthn-rs.
# The build therefore installs a C toolchain for the target, and distroless-cc provides OpenSSL at runtime with glibc, libgcc, CA roots and tzdata.
# A stage that loses its --platform=$BUILDPLATFORM pin would run emulated and stay green.
# Each guarded stage therefore fails unless it runs on the build architecture.

FROM --platform=$BUILDPLATFORM tonistiigi/xx:1.8.0 AS xx

# The bundle is architecture independent.
# It builds once on the build platform instead of once per target architecture.
FROM --platform=$BUILDPLATFORM oven/bun:1 AS webui
WORKDIR /build
COPY webui/package.json webui/bun.lock ./
RUN bun install --frozen-lockfile
COPY webui/ ./
RUN bun run build

FROM --platform=$BUILDPLATFORM lukemathwalker/cargo-chef:latest-rust-1-bookworm AS chef
COPY --from=xx / /
WORKDIR /build
# Fails with the cause when the stage runs as another architecture than the build machine.
# Debian architecture names equal Docker's BUILDARCH for amd64 and arm64.
RUN printf '%s\n' \
    '#!/bin/sh' \
    'if [ "$(dpkg --print-architecture)" != "$BUILDARCH" ]; then' \
    '  echo "this stage runs as $(dpkg --print-architecture) but the build machine is $BUILDARCH: a FROM is missing --platform=\$BUILDPLATFORM" >&2' \
    '  exit 1' \
    'fi' \
    > /usr/local/bin/assert-build-architecture \
    && chmod +x /usr/local/bin/assert-build-architecture
ARG BUILDARCH
RUN assert-build-architecture
# The C compiler driver and linker run on the build machine and target the platform through xx.
RUN apt-get update \
    && apt-get install -y --no-install-recommends clang lld \
    && rm -rf /var/lib/apt/lists/*

# Only Rust inputs enter the planner and builder stages.
# A web UI change therefore never invalidates the compiled dependency or crate layers.
FROM chef AS planner
ARG BUILDARCH
RUN assert-build-architecture
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
RUN cargo chef prepare --recipe-path recipe.json

# Fetch the dependencies once for every target platform.
FROM chef AS fetch
ARG BUILDARCH
RUN assert-build-architecture
COPY rust-toolchain.toml ./
COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --no-build --recipe-path recipe.json \
    && cargo fetch --locked

FROM fetch AS builder
ARG BUILDARCH
RUN assert-build-architecture
ARG TARGETPLATFORM
# Optional cap on parallel compile jobs, for hosts with limited memory.
ARG CARGO_BUILD_JOBS
RUN xx-apt-get install -y --no-install-recommends xx-c-essentials libssl-dev
# xx-cargo exports PKG_CONFIG=<triple>-pkg-config when that command exists, and the Debian target packages ship none.
# Without it the openssl-sys build script refuses to cross-compile.
# The wrapper looks only in the target's multiarch directory, so a native build takes the same path as a cross build.
RUN printf '%s\n' \
    '#!/bin/sh' \
    "export PKG_CONFIG_LIBDIR=/usr/lib/$(xx-info triple)/pkgconfig:/usr/share/pkgconfig" \
    'exec pkg-config "$@"' \
    > "/usr/local/bin/$(XX_VENDOR=unknown xx-info)-pkg-config" \
    && chmod +x "/usr/local/bin/$(XX_VENDOR=unknown xx-info)-pkg-config"
# Compile the dependency graph of mmcp-server only.
# This layer is reused until Cargo.toml or Cargo.lock change.
RUN xx-cargo chef cook --release --recipe-path recipe.json -p mmcp-server
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
RUN xx-cargo build --release --locked -p mmcp-server \
    && mkdir -p /out \
    && cp "target/$(xx-cargo --print-target-triple)/release/mmcp-server" /out/mmcp-server \
    && xx-verify /out/mmcp-server
# Pre-create the data directory owned by the distroless nonroot uid so
# a freshly-provisioned named volume mounted at /data/repos inherits
# that ownership (Docker copies initial contents + perms from the
# image). Distroless has no shell, so we stage this in the builder.
RUN mkdir -p /staging/data/repos

FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
COPY --from=builder /out/mmcp-server /usr/local/bin/mmcp-server
COPY --from=builder --chown=nonroot:nonroot /staging/data /data
COPY --from=webui /build/build /usr/share/mmcp/webui
# The directory must hold only the built web UI: every file in it is public.
ENV MMCP_WEBUI_DIR=/usr/share/mmcp/webui
USER nonroot
EXPOSE 8787
ENTRYPOINT ["/usr/local/bin/mmcp-server"]
