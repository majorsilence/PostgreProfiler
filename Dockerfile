# syntax=docker/dockerfile:1
#
# Stages:
#   toolchain - Rust + cargo-pgrx + Postgres 17 and 18 server/dev packages.
#               No source; used by the compose "builder" service with the repo
#               bind-mounted for compile/lint/test.
#   build     - compiles and packages the extension for PG_MAJOR.
#   postgres  - stock Postgres image with the packaged extension installed.
#
# Keep PGRX_VERSION in sync with the `pgrx` version in Cargo.toml.

ARG PG_MAJOR=18

FROM rust:1-bookworm AS toolchain
ARG PGRX_VERSION=0.20.0

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
      postgresql-17 postgresql-server-dev-17 \
      postgresql-18 postgresql-server-dev-18 \
 && rm -rf /var/lib/apt/lists/*

# Postgres refuses to run as root, and `cargo pgrx test/run` starts it, so
# build and test as an unprivileged user that may write the PG install dirs
# (cargo pgrx test installs the extension into them).
RUN rustup component add clippy rustfmt

RUN useradd --create-home --uid 1000 dev \
 && install -d -o dev -g dev /src /src/target \
 && for v in 17 18; do \
      chown -R dev:dev /usr/lib/postgresql/$v/lib /usr/share/postgresql/$v/extension; \
    done
USER dev
ENV USER=dev \
    CARGO_HOME=/home/dev/.cargo \
    PATH=/home/dev/.cargo/bin:/usr/local/cargo/bin:$PATH
WORKDIR /src

RUN cargo install --locked cargo-pgrx --version ${PGRX_VERSION} \
 && cargo pgrx init \
      --pg17 /usr/lib/postgresql/17/bin/pg_config \
      --pg18 /usr/lib/postgresql/18/bin/pg_config

FROM toolchain AS build
ARG PG_MAJOR
COPY --chown=dev:dev . .
# Produces target/release/pg_profiler-pg${PG_MAJOR}/ laid out as an install tree.
RUN cargo pgrx package --no-default-features --features pg${PG_MAJOR} \
      --pg-config /usr/lib/postgresql/${PG_MAJOR}/bin/pg_config

FROM postgres:${PG_MAJOR}-bookworm AS postgres
ARG PG_MAJOR
COPY --from=build /src/target/release/pg_profiler-pg${PG_MAJOR}/ /

# Hooks and shared memory need the library preloaded at server start.
CMD ["postgres", "-c", "shared_preload_libraries=pg_profiler"]
