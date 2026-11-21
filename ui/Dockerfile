# syntax=docker/dockerfile:1
#
# pg_profiler web UI: builds the Vue app (and runs its tests), embeds it in the
# Rust server (and runs its tests), and ships the one binary.
#
#   docker build -t pg_profiler_ui ui/
#
# Stages:
#   web    - npm ci, unit tests, production build of the single-page app
#   server - cargo test and release build with the app embedded
#   (final) - a slim image running as an unprivileged user

FROM node:22-bookworm-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY web/ ./
RUN npm test && npm run build

FROM rust:1-bookworm AS server
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY server ./server
COPY --from=web /web/dist ./web/dist
RUN cargo test --release --locked && cargo build --release --locked

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --no-create-home profiler-ui
COPY --from=server /app/target/release/pg_profiler_ui /usr/local/bin/pg_profiler_ui
USER profiler-ui
EXPOSE 8080
ENV UI_LISTEN=0.0.0.0:8080
HEALTHCHECK --interval=15s --timeout=3s --retries=3 \
  CMD ["bash", "-c", "exec 3<>/dev/tcp/127.0.0.1/8080 && printf 'GET /healthz HTTP/1.0\\r\\n\\r\\n' >&3 && grep -q ok <&3"]
ENTRYPOINT ["pg_profiler_ui"]
