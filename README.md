# PostgreProfiler

A Postgres query-tracing extension written in Rust with [pgrx](https://github.com/pgcentralfoundation/pgrx).

## Development (Docker)

Both the Rust build and Postgres run in Docker; nothing needs to be installed
locally besides Docker.

```sh
# Compile, lint and test inside the Rust + pgrx container
docker compose run --rm builder cargo clippy -- -D warnings
docker compose run --rm builder cargo pgrx test pg18

# Build the extension into a Postgres image and run it
docker compose up --build db
psql postgres://postgres:postgres@localhost:5432/postgres
```

## License

Licensed under any of the following, at your option:

- MIT license ([LICENSE-MIT](LICENSE-MIT))
- BSD 3-Clause license ([LICENSE-BSD-3-Clause](LICENSE-BSD-3-Clause))
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- PostgreSQL License ([LICENSE-PostgreSQL](LICENSE-PostgreSQL))

SPDX identifier: `MIT OR BSD-3-Clause OR Apache-2.0 OR PostgreSQL`

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project shall be licensed as above, without any
additional terms or conditions.
