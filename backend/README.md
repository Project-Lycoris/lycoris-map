# Lycoris API

Rust, Axum, and SQLx, backed by PostgreSQL/PostGIS and Redis. Run Cargo from `backend/` so rustup selects `rust-toolchain.toml`. Dependencies and offline SQL metadata are committed.

## Local development

Start dependencies from the repository root:

```sh
docker compose -f backend/compose.test.yml up -d --wait
```

The pinned PostGIS image is amd64-only. Compose explicitly selects `linux/amd64`,
including on Apple Silicon; Redis and the Rust compiler can still run natively.
Keep the image digest when using an approved registry mirror.

Then, in a shell at the repository root:

```sh
cd backend
export DATABASE_URL='postgres://lycoris:lycoris_local_test@127.0.0.1:55432/lycoris_rust'
export REDIS_URL='redis://127.0.0.1:56379'
export WRITE_ALLOWED_ORIGINS='http://localhost:5173,http://127.0.0.1:5173'
export SQLX_OFFLINE=true
cargo run --locked -- --migrate
cargo run --locked
```

In PowerShell, set the same variables with `$env:NAME = 'value'`. The binary reads process environment variables; it does not load `.env` automatically. See [.env.example](.env.example) for all settings. These database credentials are for the local synthetic environment only.

The API listens on `127.0.0.1:8080` by default. `/health/live` checks the process; `/health/ready` checks PostgreSQL and Redis. `/api/markers/public` returns an empty list on a fresh database. Relative upload paths resolve from the working directory.

Registration and password recovery require SMTP plus an independent `EMAIL_VERIFICATION_SECRET`. Without mail configuration, the API rejects code requests rather than allowing unverified registration. See [email verification](deploy/production/EMAIL_VERIFICATION.md).

## Administration

Admin endpoints authorize the current database role `ADMIN` on every request. Account login is the only credential required. The retired `ADMIN_SECOND_FACTOR_ENABLED`, `ADMIN_SECOND_PASSWORD_HASH`, and `SECOND_FACTOR_TTL_SECONDS` settings are ignored; existing environment files and sessions need no migration. `/api/admin/verify` remains a role-checked compatibility endpoint for cached clients.

The former `/api/admin/markers/cleanup-missing-images` action returns 410 without modifying any records or files, including when called by a cached client.

Deploy this backend before the matching Web update so cached clients and the new role-only entry both work during rollout.

## Database changes

Normal startup verifies migration history without changing the schema. Apply pending migrations explicitly with `cargo run --locked -- --migrate`. Never edit a migration already applied to any shared database; add another migration instead.

For an older database with application tables but no SQLx history, use `--check-baseline` for a read-only check, then `--adopt-baseline` to register the verified baseline before `--migrate`. Back up existing data first. Adoption does not recreate business tables.

Migrations `0009`–`0011` add ordered facility types, opening-hours notes, published photo albums, and the park venue. Existing `category`/`markImage` remain compatible with older clients. The last migration only reclassifies eight reviewed records when their ID, title, and version still match. Back up the database and uploads, apply these migrations, and deploy the API before clients start sending the new fields.

Migration `0012` widens marker, edit-proposal and photo-proposal attribution names
to the existing account limit of 255 characters. Apply it before deploying this API;
existing long usernames and old attribution values are preserved without renaming or
truncation. Account IDs, permissions and historical migration checksums do not change.

After changing SQL queries, point `DATABASE_URL` at a migrated synthetic database and regenerate metadata with SQLx CLI 0.9.0:

```sh
unset SQLX_OFFLINE
cargo sqlx prepare -- --all-targets
```

Commit the updated `.sqlx/` files with the queries. Use `SQLX_OFFLINE=true` for builds that should not connect to a database.

## Checks

With the local test services running, from `backend/`:

```sh
export SQLX_OFFLINE=true
export RUST_TEST_THREADS=4
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

Integration tests create and remove their own temporary databases and Redis namespaces. They require loopback synthetic services; unavailable services fail the tests. Avoid running full suites from several worktrees against the same small database container.

`pwsh scripts/check-rust.ps1` also checks offline SQL metadata. For the container test runner, use `docker compose -f compose.release.yml run --build --rm test`; its target guards restrict access to the local test dependencies.

## Runtime structure

Routes in `src/app.rs` compose account, marker, admin, and media services. Fixed SQL lives in `queries/` and module `sql/` directories. The [architecture guide](../ARCHITECTURE.md) describes visibility, session, coordinate, and upload boundaries.

Public place reads require approved, public, non-deactivated records. Admin removal is reversible deactivation. Image access is checked against the current record and viewer; pending/private files are not static public assets.

API responses retain their established per-route shapes: envelopes, plain JSON, text, or empty bodies. Client adapters must handle these explicitly rather than assuming every route has the same envelope.

## Deployment

Build the runtime image from `backend/`:

```sh
docker build --target runtime -t lycoris-backend:local .
```

The runtime runs as a non-root user. Keep uploads on a writable persistent volume, and keep credentials outside the image. `compose.release.yml` is for local container checks; [Shanghai production configuration](deploy/shanghai/README.md) covers the shared backend for both websites, HTTPS, migrations, and backups.
