# syntax=docker/dockerfile:1.7

# Build the web UI as a static SvelteKit bundle and the mmcp-server binary with cargo-chef dependency caching.
# Both are copied into a distroless runtime image.
# The server serves the bundle on its own origin from MMCP_WEBUI_DIR.
# The workspace is pure Rust (gix, rustls, sqlx with bundled sqlite).
# distroless-cc therefore provides everything needed at runtime: libc, libgcc, CA roots, tzdata.

# The bundle is architecture independent.
# It builds once on the build platform instead of once per target architecture under emulation.
FROM --platform=$BUILDPLATFORM oven/bun:1 AS webui
WORKDIR /build
COPY webui/package.json webui/bun.lock ./
RUN bun install --frozen-lockfile
COPY webui/ ./
RUN bun run build

FROM lukemathwalker/cargo-chef:latest-rust-1-bookworm AS chef
WORKDIR /build

# Only Rust inputs enter the planner and builder stages.
# A web UI change therefore never invalidates the compiled dependency or crate layers.
FROM chef AS planner
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /build/recipe.json recipe.json
# Cook the dependency graph for mmcp-server only. This layer is cached
# until Cargo.toml / Cargo.lock change.
RUN cargo chef cook --release --recipe-path recipe.json -p mmcp-server
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
RUN cargo build --release --locked -p mmcp-server
# Pre-create the data directory owned by the distroless nonroot uid so
# a freshly-provisioned named volume mounted at /data/repos inherits
# that ownership (Docker copies initial contents + perms from the
# image). Distroless has no shell, so we stage this in the builder.
RUN mkdir -p /staging/data/repos

FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
COPY --from=builder /build/target/release/mmcp-server /usr/local/bin/mmcp-server
COPY --from=builder --chown=nonroot:nonroot /staging/data /data
COPY --from=webui /build/build /usr/share/mmcp/webui
# The directory must hold only the built web UI: every file in it is public.
ENV MMCP_WEBUI_DIR=/usr/share/mmcp/webui
USER nonroot
EXPOSE 8787
ENTRYPOINT ["/usr/local/bin/mmcp-server"]
