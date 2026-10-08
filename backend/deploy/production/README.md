# Previous backend deployment

Production moved to [Shanghai](../shanghai/README.md) on 2026-10-08. This directory
retains the previous host's configuration and shared initialization helpers.
Do not restart its application or use its historical database for a new release.

This Compose stack runs the Rust API, PostgreSQL/PostGIS, Redis, and Caddy. The API binds to loopback port `18081`; Caddy serves `https://api.lycoris-map.com`. PostgreSQL and Redis bind only to loopback on `15432` and `16379`.

## Host layout

| Path                   | Contents                                        |
| ---------------------- | ----------------------------------------------- |
| `/opt/lycoris/source`  | Checked-out release source                      |
| `/opt/lycoris/private` | Database credentials, `app.env`, and `smtp.env` |
| `/opt/lycoris/data`    | PostgreSQL, Redis, uploads, and Caddy state     |
| `/opt/lycoris/backups` | Private backups and restore records             |

Use a private deployment `.env` to set `LYCORIS_APP_IMAGE` to the exact release image tag. Keep configuration and backup files outside Git. Compose loads `app.env` and `smtp.env` in raw format; SMTP setup is described in [EMAIL_VERIFICATION.md](EMAIL_VERIFICATION.md).

`prepare.sh` initializes an empty host using an existing private admin configuration and refuses to overwrite credentials. `init-database.sh` creates the application role on first database initialization. Existing hosts retain their credentials and data directories.

## Release procedure

1. Build the runtime image from the intended commit with `docker build --target runtime`, using one Cargo job on small hosts. Retain the running image and private configuration for recovery.
2. Prevent concurrent deployments. Pause application writes and back up PostgreSQL, roles, Redis, uploads, and configuration. Verify that the database dump can be read.
3. Run the new image with `--migrate` explicitly against the intended database. Normal startup never applies schema changes. Inspect migration history before switching the service.
4. Set `LYCORIS_APP_IMAGE` to the verified image and recreate only the app service. Check `/health/ready` locally and through the public HTTPS route, then verify the affected user flows.
5. Keep the previous image and backup until the release is accepted. An application rollback must remain compatible with the applied schema; do not automatically restore an older database over newer user writes.

Do not expose `/opt/lycoris/data/uploads` as a static web directory: the Rust route enforces visibility for originals and thumbnails. Caddy's persisted state holds HTTPS certificates. Trust forwarded client addresses only from the configured proxies.

## Backup and recovery

Include PostgreSQL logical dumps and roles, Redis persistence, uploads, private configuration, Caddy state, and the release image/source identifier. Database and file snapshots must agree; pause writes while capturing them. A filesystem copy of PostgreSQL must be made with the database stopped or through a database-aware backup mechanism.

Store checksums with each backup and verify the downloaded copy. Test `pg_restore --exit-on-error` against an empty, isolated PostgreSQL/PostGIS database and compare migration history and table contents before relying on it. Never use a production database for a restore rehearsal.

Original images remain on the host; Cloudflare R2 is a delivery copy. External Cloudflare configuration and R2 data require their own backup scope. See the [Pages deployment guide](../../../frontend/deploy/cloudflare/README.md) for edge routing and media authorization.
