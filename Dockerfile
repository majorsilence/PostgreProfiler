# syntax=docker/dockerfile:1
#
# Stage "builder": Rust toolchain + cargo-pgrx + Postgres dev headers.
#   Used for compiling/testing (see docker-compose.yml, service "builder").
# Stage "postgres": stock Postgres image with the compiled extension installed.
#
# Keep PGRX_VERSION in sync with the `pgrx` version in Cargo.toml, and
# EXT_NAME in sync with the crate / control-file name.

ARG PG_MAJOR=17

FROM rust:1-bookworm AS builder
ARG PG_MAJOR
ARG PGRX_VERSION=0.17.0
ARG EXT_NAME=pg_trace

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl gnupg lsb-release \
      build-essential clang libclang-dev pkg-config libssl-dev \
      libreadline-dev zlib1g-dev bison flex \
 && install -d /usr/share/postgresql-common/pgdg \
 && curl -fsSL -o /usr/share/postgresql-common/pgdg/apt.postgresql.org.asc \
      https://www.postgresql.org/media/keys/ACCC4CF8.asc \
 && echo "deb [signed-by=/usr/share/postgresql-common/pgdg/apt.postgresql.org.asc] https://apt.postgresql.org/pub/repos/apt $(lsb_release -cs)-pgdg main" \
      > /etc/apt/sources.list.d/pgdg.list \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
      postgresql-${PG_MAJOR} postgresql-server-dev-${PG_MAJOR} \
 && rm -rf /var/lib/apt/lists/*

# Postgres refuses to run as root, and `cargo pgrx test/run` starts it,
# so build and test as an unprivileged user.
RUN useradd --create-home --uid 1000 dev \
 && install -d -o dev -g dev /src
USER dev
ENV CARGO_HOME=/home/dev/.cargo \
    PATH=/home/dev/.cargo/bin:/usr/local/cargo/bin:$PATH
WORKDIR /src

RUN cargo install --locked cargo-pgrx --version ${PGRX_VERSION} \
 && cargo pgrx init --pg${PG_MAJOR} /usr/lib/postgresql/${PG_MAJOR}/bin/pg_config

COPY --chown=dev:dev . .

# Produces target/release/${EXT_NAME}-pg${PG_MAJOR}/ laid out as an install tree.
RUN cargo pgrx package --pg-config /usr/lib/postgresql/${PG_MAJOR}/bin/pg_config

FROM postgres:${PG_MAJOR}-bookworm AS postgres
ARG PG_MAJOR
ARG EXT_NAME=pg_trace

COPY --from=builder /src/target/release/${EXT_NAME}-pg${PG_MAJOR}/ /

# Hooks and shared memory need the library preloaded at server start.
CMD ["postgres", "-c", "shared_preload_libraries=pg_trace"]
