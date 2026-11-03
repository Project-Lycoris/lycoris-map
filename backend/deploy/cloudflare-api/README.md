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

Store a random value of at least 32 characters as the encrypted Worker secret
`EDGE_SECRET` (`wrangler secret put EDGE_SECRET`). The Worker sends it in
`X-Lycoris-Edge-Secret`, and Shanghai trusts `X-Forwarded-For` only when it
matches `LYCORIS_EDGE_SECRET`. Without the secret the Worker returns 502 instead
of merging every user into one rate-limit identity. Rotate it by updating the
Worker first, then Shanghai; requests briefly use the Worker's address meanwhile.

Run `node --test backend/deploy/cloudflare-api/worker.test.mjs` before publishing.
Publish without the production route first, verify Shanghai, and attach the route
only after the final data copy. Set the route to fail closed; never silently fall
back to a stopped or stale backend. Keep a proxied DNS record for the API hostname.
