# syntax=docker/dockerfile:1.7

# Build the mmcp-server binary with cargo-chef dependency caching, then
# copy it into a distroless runtime image. The workspace is pure Rust
# (gix, rustls, sqlx with bundled sqlite) so distroless-cc provides
# everything needed at runtime: libc, libgcc, CA roots, tzdata.

FROM lukemathwalker/cargo-chef:latest-rust-1-bookworm AS chef
WORKDIR /build

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /build/recipe.json recipe.json
# Cook the dependency graph for mmcp-server only. This layer is cached
# until Cargo.toml / Cargo.lock change.
RUN cargo chef cook --release --recipe-path recipe.json -p mmcp-server
COPY . .
RUN cargo build --release --locked -p mmcp-server
# Pre-create the data directory owned by the distroless nonroot uid so
# a freshly-provisioned named volume mounted at /data/repos inherits
# that ownership (Docker copies initial contents + perms from the
# image). Distroless has no shell, so we stage this in the builder.
RUN mkdir -p /staging/data/repos

FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
COPY --from=builder /build/target/release/mmcp-server /usr/local/bin/mmcp-server
COPY --from=builder --chown=nonroot:nonroot /staging/data /data
USER nonroot
EXPOSE 8787
ENTRYPOINT ["/usr/local/bin/mmcp-server"]
