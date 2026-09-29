# Cloudflare Pages deployment

The `lycoris-map` Pages project builds `frontend/` from `Project-Lycoris/lycoris-map` on `main` and serves `https://lycoris-map.com`. Other branches produce preview deployments. Output is `dist/`; the worker files are copied into that output by the build command.

The previous `lycoris-main` project retains its deployment history and the retired `lycoris.online` domain bindings. Those domains redirect to the current site; they are not the production build target.

## Build configuration

Use the Node and pnpm versions pinned in `frontend/.nvmrc` and `frontend/package.json`. Pages installs dependencies before running:

```sh
pnpm test:unit && node --test deploy/cloudflare/worker.test.js && pnpm build && cp deploy/cloudflare/_worker.js deploy/cloudflare/_routes.json deploy/cloudflare/_headers dist/
```

Configure these optional browser keys separately for Production and Preview:

| Variable                | Provider                            |
| ----------------------- | ----------------------------------- |
| `VITE_TENCENT_MAP_KEY`  | Tencent JavaScript API GL           |
| `VITE_TIANDITU_API_KEY` | Tianditu vector and annotation WMTS |

These keys enter the browser bundle; use provider restrictions and rebuild after changes. Server credentials must never use `VITE_` variables. The client uses same-origin routes, so no API base URL build variable is needed.

The Worker serves SPA assets and proxies API, upload, and health routes to the fixed HTTPS API origin. Preview requests use that backend too; previews are not in the production write-origin allowlist. Use local synthetic services for write tests.

## Map tiles

`/tiles/osm/{z}/{x}/{y}.png` fetches from a fixed OSM tile upstream, with attribution, an identifying User-Agent, bounded inputs, and provider-aware caching. It is not a general URL proxy or a bulk/offline tile downloader. Tencent and Tianditu use their provider adapters and configured browser keys.

Language defaults and failure recovery live in the frontend map-source module. An explicit available user choice takes precedence; without one, Chinese tries Tencent → Tianditu → OSM and English tries OSM → Tencent → Tianditu. Idle adapter loading must not trigger bulk tile requests.

## Images and R2

Bind `MEDIA_BUCKET` to `lycoris-media-prod` in Production and `lycoris-media-preview` in Preview. Keep both R2 buckets private with public access disabled. The binding supplies Worker access; no S3 credentials belong in the browser or Rust image. Redeploy after changing bindings so the running Worker receives the updated configuration.

For every media request, the Worker first makes an uncached authorization HEAD request to Rust using the current viewer's credentials. It then uses the authorized content hash to read the edge cache or R2. Authorization failure or origin unavailability must never fall back to cached bytes.

On a cache miss, the Worker fetches the image with `If-Match`, validates the content hash, and populates R2 on demand. Public reads can therefore warm the delivery cache without a migration script. An R2 failure can fall back to an authorized origin response. Browser responses remain private and non-cacheable; internal immutable copies contain no session cookies.

The backend retains originals and `thumb` / `detail` renditions for recovery. Files above the Worker's buffering limit stream from the origin. R2 is a delivery copy, and deactivating a place changes access without physically deleting stored objects.

## Domain and release checks

Pages owns the Web domain; Caddy terminates HTTPS for the API origin. Keep the Worker upstream on HTTPS and the backend write-origin allowlist aligned with the production site. The retired domain redirects to the current site.

After a deployment, check the SPA, `/health/ready`, public places, and all configured map sources. Verify anonymous and authenticated media access, including a denied request after a place becomes private or deactivated. Build success alone does not validate provider keys or device permissions.
