# syntax=docker/dockerfile:1.7

# Build the mmcp-webui Leptos SSR binary via cargo-leptos, which also
# compiles the WASM hydrate bundle and the SCSS stylesheet. Both the
# binary and the generated site/ directory are copied into the runtime
# image; Leptos reads LEPTOS_SITE_ROOT at startup to locate static
# assets. The build context is the webui/ directory because webui is a
# separate Cargo project (excluded from the root workspace).

FROM lukemathwalker/cargo-chef:latest-rust-1 AS chef
WORKDIR /build
# cargo-binstall avoids a cold compile of cargo-leptos from source.
RUN curl -L --proto '=https' --tlsv1.2 -sSf \
      https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh \
    | bash \
 && cargo binstall -y --no-confirm cargo-leptos \
 && rustup target add wasm32-unknown-unknown

FROM chef AS planner
COPY webui/ .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY webui/ .
RUN cargo leptos build --release

FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
WORKDIR /app
COPY --from=builder /build/target/release/mmcp-webui /app/mmcp-webui
COPY --from=builder /build/target/site /app/site
ENV LEPTOS_SITE_ROOT=/app/site \
    LEPTOS_SITE_ADDR=0.0.0.0:3000 \
    LEPTOS_ENV=PROD \
    LEPTOS_OUTPUT_NAME=mmcp-webui
USER nonroot
EXPOSE 3000
ENTRYPOINT ["/app/mmcp-webui"]
