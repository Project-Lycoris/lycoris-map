# Shanghai production

`lycoris-map.cn` runs Web, Rust, PostgreSQL/PostGIS and Redis on `111.229.9.16`.
Shanghai is the sole write authority. The overseas Web remains on Cloudflare Pages;
`api.lycoris-map.com` is a Cloudflare Worker route to the same Shanghai API, preserving
the URL used by installed native apps. See `../cloudflare-api/worker.js`.

Cutover completed on **2026-10-08**. The active database is
`lycoris_import_2026100802`, with originals in `/opt/lycoris/data/uploads-final-20261008`.
The previous application is stopped with container restart disabled. Both Web
domains and the native API were checked against the same public marker IDs;
search, nearby results, details, images, authorization and HTTPS redirects passed.
The complete source snapshot, final Shanghai dump and private configuration are
also retained in the operator's local `Documents/Lycoris-Backups` directory.

## Host layout

- Release: `/opt/lycoris/releases/shanghai-20261008` (backend from `ded3b83`).
- Private configuration: `/opt/lycoris/private/{app,smtp,deploy}.env`, mode 0600.
- PostgreSQL/Redis and original images: `/opt/lycoris/data`.
- Migration snapshots and previous configurations: `/opt/lycoris/backups`.
- Caddy serves public HTTPS and redirects HTTP; nginx serves static Web files on
  loopback 18080. Rust, PostgreSQL and Redis bind only to loopback.
- `LYCORIS_GATEWAY_IMAGE` can pin the verified imported Caddy image ID when the
  registry is unreachable. Backend and frontend releases are explicitly pinned.

Compose uses raw environment files. Do not print expanded Compose configuration or
commit private files. `SESSION_COOKIE_SECURE=true`; the exact write origins are
`https://lycoris-map.cn`, `https://www.lycoris-map.cn`, and `https://lycoris-map.com`.
Cookie domains are unset, keeping each site's browser session host-only. Accounts,
password hashes, favorites, moderation records and uploads share the same database.
Old sessions are not carried over; users sign in again after the cutover.

## Copy and cutover

1. Back up both hosts' configuration and databases. Compare the previous Shanghai
   snapshot to ensure it contains no unmerged private edits.
2. `copy-snapshot.py --output /opt/lycoris/backups/<batch>` captures a consistent
   source PostgreSQL snapshot, full table fingerprints and immutable original
   images. Photo albums and resumable-upload bytes are included. Redis and session
   tokens are deliberately excluded. Transfer privately and verify SHA256SUMS.
3. Restore to a **new** `lycoris_import_<digits>` database and new media directory
   with `restore-copy.py`; verify with `verify-copy.py` before running the app.
   During final cutover, `--media-source` reuses previously copied immutable
   originals after checking every size and SHA-256. Transfer new originals first;
   retain and verify the complete final archive separately as a recovery backup.
4. Run the new image's explicit `--migrate`, preserving a pre-migration dump.
   Normal startup only validates schema compatibility.
5. Rehearse readiness, public/search/nearby/detail/media, denied writes, and SMTP
   TLS/authentication. Before enabling both public sites, stop source application
   writes, capture the final database and any newly added originals, then repeat
   restoration and validation. Never activate two independently writable copies.
6. Start Shanghai with the final imported database. Activate the API Worker route,
   verify both domains and native API requests, and leave the old application
   stopped. Retain the old database and images as historical recovery material.

The snapshot helpers describe the one-time Korea-to-Shanghai migration. For later
releases, back up the **active Shanghai database** and upload directory, explicitly
run the new image's `--migrate`, then recreate only the app service with this stack's
`compose.yml` and `/opt/lycoris/private/deploy.env`. Do not deploy against the retired
Korea stack or copy its historical database over new Shanghai writes.

An application rollback must preserve new user writes. Do not blindly restore the
old source database after public cutover, drop volumes, or lower ID sequences.

## Domain, filing and TLS

The apex and `www` DNS records resolve to Shanghai. The Web footer shows
`辽ICP备2026022983号-1` on these hosts and links to `https://beian.miit.gov.cn/`.
The footer reserves its measured height, including the phone safe area, so it
cannot cover bottom-sheet actions. Other domains do not display this filing.

The supplied certificate covers both `.cn` hostnames and expires on
**2026-12-22 00:59:59 Asia/Shanghai**. It is stored under
`/opt/lycoris/private/tls/lycoris-map.cn/`; automatic renewal is not configured.
Replace the certificate/key together, verify their match, and recreate Caddy and
nginx because individual bind mounts can retain the old file inode.

Only TCP 80/443 are public Web ports. Cloudflare client-address headers are trusted
only from Cloudflare's published IP ranges; direct clients cannot spoof them.
The OSM route remains a bounded, cached tile relay through the existing Worker.
Tencent and Tianditu browser keys remain deployment inputs. Mainland media is
served by Rust's authorized upload routes; `.com` retains its existing authorized
R2 delivery cache.

[Official filing-link guidance](https://cloud.tencent.com/document/product/243/61412).
Add the separate public-security filing only after its own number is issued.
