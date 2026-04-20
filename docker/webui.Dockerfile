# syntax=docker/dockerfile:1.7

# Build the mmcp webui as a SvelteKit + Tailwind static SPA. Output is
# a plain `build/` directory of hashed assets served by `nginx:alpine`
# at runtime. No Node process lives in the runtime image — Tailwind v4
# + adapter-static emit pure HTML/CSS/JS.
#
# Build arg `VITE_MMCP_SERVER_URL` bakes the upstream server URL into
# the bundle at compile time; leave unset to let the client fall back
# to http://127.0.0.1:8787, handy for dev.

FROM oven/bun:1 AS builder
WORKDIR /build
COPY webui/package.json webui/bun.lock* ./
RUN bun install --frozen-lockfile || bun install
COPY webui/ ./
ARG VITE_MMCP_SERVER_URL
ENV VITE_MMCP_SERVER_URL=${VITE_MMCP_SERVER_URL}
RUN bun run build

FROM nginx:alpine AS runtime
# SPA fallback: every unknown path serves `index.html` so the
# SvelteKit client router can pick it up on hydrate.
RUN printf 'server {\n\
  listen 3000;\n\
  root /usr/share/nginx/html;\n\
  index index.html;\n\
  location / {\n\
    try_files $uri $uri/ /index.html;\n\
  }\n\
}\n' > /etc/nginx/conf.d/default.conf
COPY --from=builder /build/build /usr/share/nginx/html
EXPOSE 3000
CMD ["nginx", "-g", "daemon off;"]
