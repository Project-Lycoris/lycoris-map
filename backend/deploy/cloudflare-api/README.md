# Shared API gateway

Deploy `worker.js` as the `lycoris-api-shanghai` Cloudflare Worker with route
`api.lycoris-map.com/*` in the `lycoris-map.com` zone. Disable its workers.dev and
version-preview URLs after route verification. The fixed origin is
`https://lycoris-map.cn`; no origin, credentials or API keys are accepted from callers.

The Worker preserves methods, bodies, cookies, Origin, bearer tokens and response
cookies, and replaces forwarded-address headers. It does not follow redirects or
cache responses. Backend authentication and origin checks remain authoritative.
Keep `X-Lycoris-Media-Key` intact: the existing Pages Worker uses it to authorize
access to its private R2 image copies.

Run `node --test backend/deploy/cloudflare-api/worker.test.mjs` before publishing.
Publish without the production route first, verify Shanghai, and attach the route
only after the final data copy. Set the route to fail closed; never silently fall
back to a stopped or stale backend. Keep a proxied DNS record for the API hostname.
