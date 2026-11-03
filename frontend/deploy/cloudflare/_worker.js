// Server-side Pages entry point. Never bundle this file into the browser app.
const BACKEND_ORIGIN = 'https://api.lycoris-map.com'
const MEDIA_PATH = /^\/uploads\/(avatars|markers)\/[A-Za-z0-9_.-]+\.(?:jpe?g|png|webp|gif)$/i
const MEDIA_KEY = /^media\/v1\/([a-f0-9]{64})\.(jpg|png|webp|gif)$/
const MAX_IMAGE_BYTES = 16 * 1024 * 1024
const OSM_TILE = /^\/tiles\/osm\/(0|[1-9]\d?)\/(0|[1-9]\d*)\/(0|[1-9]\d*)\.png$/

async function osmTile(request) {
    const url = new URL(request.url)
    const match = url.pathname.match(OSM_TILE)
    const [z, x, y] = match ? match.slice(1).map(Number) : []
    const fail = (status) =>
        new Response(null, {
            status,
            headers: { 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff' },
        })
    if (!match || z > 19 || x >= 2 ** z || y >= 2 ** z || url.search) return fail(404)
    if (!['GET', 'HEAD'].includes(request.method)) {
        const response = fail(405)
        response.headers.set('Allow', 'GET, HEAD')
        return response
    }
    // Fixed provider/path only: never forward cookies, credentials or arbitrary URLs.
    // OSM requires stable application identification and a Referer. Send only the
    // site origin: the page URL carries marker IDs, coordinates and shared titles.
    const headers = new Headers({
        'User-Agent': 'LycorisMaps/1.0 (+https://lycoris-map.com)',
        Accept: 'image/png',
        Referer: 'https://lycoris-map.com/',
    })
    for (const name of ['If-None-Match', 'If-Modified-Since']) {
        const value = request.headers.get(name)
        if (value) headers.set(name, value)
    }
    try {
        const upstream = await fetch(
            new Request(`https://tile.openstreetmap.org/${z}/${x}/${y}.png`, {
                method: request.method,
                headers,
                signal: AbortSignal.timeout(15000),
            }),
            {
                redirect: 'manual',
                // Cloudflare's fetch cache honors the provider's freshness/validators.
                // Do not override its TTL or forward browser reload/cache-bypass headers.
                cf: { cacheEverything: true },
            },
        )
        if (
            upstream.status !== 304 &&
            (upstream.status !== 200 ||
                upstream.headers.get('Content-Type')?.split(';')[0] !== 'image/png' ||
                upstream.headers.has('x-blocked'))
        ) {
            await upstream.body?.cancel()
            return fail(upstream.status === 404 ? 404 : 502)
        }
        const outgoing = new Headers()
        for (const name of [
            'Content-Type',
            'Content-Length',
            'Cache-Control',
            'Expires',
            'ETag',
            'Last-Modified',
            'Age',
            'Date',
        ]) {
            const value = upstream.headers.get(name)
            if (value) outgoing.set(name, value)
        }
        outgoing.set('X-Content-Type-Options', 'nosniff')
        outgoing.set('X-Lycoris-Tile-Source', 'osm-worker')
        outgoing.set('X-Lycoris-Tile-Cache', upstream.headers.get('CF-Cache-Status') ?? 'UNKNOWN')
        return new Response(upstream.body, { status: upstream.status, headers: outgoing })
    } catch {
        return fail(502)
    }
}

function backendRequest(request, method = request.method) {
    const incoming = new URL(request.url)
    const upstream = new URL(BACKEND_ORIGIN)
    upstream.pathname = incoming.pathname
    upstream.search = incoming.search
    const forwarded = new Request(upstream, request)
    forwarded.headers.set('X-Forwarded-Host', incoming.host)
    forwarded.headers.set('X-Forwarded-Proto', 'https')
    // A same-zone subrequest derives CF-Connecting-IP from X-Real-IP. Pin both to the
    // visitor seen by Cloudflare so callers cannot choose their rate-limit identity.
    const client = request.headers.get('CF-Connecting-IP') || ''
    forwarded.headers.delete('Forwarded')
    forwarded.headers.set('X-Real-IP', client)
    forwarded.headers.set('X-Forwarded-For', client)
    return method === request.method ? forwarded : new Request(forwarded, { method, body: null })
}

const originOptions = { redirect: 'manual', cf: { cacheTtl: 0, cacheEverything: false } }

function privateResponse(response) {
    const result = new Response(response.body, response)
    result.headers.set('Cache-Control', 'private, no-store')
    result.headers.delete('x-lycoris-media-key')
    return result
}

function imageHeaders(metadata, source) {
    const headers = new Headers(metadata.headers)
    for (const name of [
        'x-lycoris-media-key',
        'cf-cache-status',
        'age',
        'content-encoding',
        'transfer-encoding',
    ])
        headers.delete(name)
    headers.set('Cache-Control', 'private, no-store')
    headers.set('X-Content-Type-Options', 'nosniff')
    headers.set('X-Lycoris-Media-Source', source)
    return headers
}

function later(context, operation) {
    const safe = operation.catch(() => {
        /* Cache/storage failure must not break a valid image. */
    })
    if (context?.waitUntil) context.waitUntil(safe)
    return safe
}

async function controlledImage(request, env, context) {
    // Never cache the permission response. Even warm R2/edge hits must recheck
    // current visibility and session; fail closed on logout, revocation or outage.
    const permission = backendRequest(request, 'HEAD')
    for (const header of [
        'If-None-Match',
        'If-Modified-Since',
        'If-Match',
        'If-Unmodified-Since',
        'Range',
        'If-Range',
    ])
        permission.headers.delete(header)
    const metadata = await fetch(permission, originOptions)
    if (metadata.status !== 200) {
        const headers = imageHeaders(metadata, 'denied')
        headers.delete('Content-Length')
        return new Response(null, {
            status: metadata.status === 304 ? 503 : metadata.status,
            headers,
        })
    }
    const key = metadata.headers.get('x-lycoris-media-key') ?? ''
    const match = key.match(MEDIA_KEY)
    const etag = metadata.headers.get('ETag')
    const length = Number(metadata.headers.get('Content-Length'))
    const mime = { jpg: 'image/jpeg', png: 'image/png', webp: 'image/webp', gif: 'image/gif' }
    if (
        !match ||
        etag !== `"${match[1]}"` ||
        metadata.headers.get('Content-Type') !== mime[match[2]] ||
        !Number.isSafeInteger(length) ||
        length <= 0 ||
        length > MAX_IMAGE_BYTES
    ) {
        // Safe rollout against an older backend; never infer a storage key from user input.
        return privateResponse(await fetch(backendRequest(request), originOptions))
    }
    const headers = imageHeaders(metadata, 'authorized')
    const ifMatch = request.headers.get('If-Match')
    if (ifMatch && ifMatch !== '*' && !ifMatch.split(',').some((value) => value.trim() === etag)) {
        headers.delete('Content-Length')
        return new Response(null, { status: 412, headers })
    }
    const unchanged = request.headers
        .get('If-None-Match')
        ?.split(',')
        .some((value) => value.trim() === '*' || value.trim().replace(/^W\//, '') === etag)
    if (unchanged) {
        headers.delete('Content-Length')
        return new Response(null, { status: 304, headers })
    }
    if (request.method === 'HEAD') return new Response(null, { headers })

    // Range is optional for these small still images. Return the full representation
    // with 200 consistently, never put a partial response under the complete key.
    const cache = globalThis.caches?.default
    const cacheKey = new Request(new URL(`/__lycoris_media_cache/${key}`, request.url))
    try {
        const hit = await cache?.match(cacheKey)
        if (
            hit &&
            hit.headers.get('ETag') === etag &&
            Number(hit.headers.get('Content-Length')) === length
        ) {
            headers.set('X-Lycoris-Media-Source', 'edge')
            return new Response(hit.body, { headers })
        }
    } catch {
        /* An unavailable edge cache falls through to R2. */
    }

    let response
    try {
        const object = await env.MEDIA_BUCKET.get(key)
        if (object?.body && object.size === length && object.customMetadata?.sha256 === match[1]) {
            headers.set('X-Lycoris-Media-Source', 'r2')
            response = new Response(object.body, { headers })
        }
    } catch {
        /* Retain origin fallback when R2 is temporarily unavailable. */
    }
    if (!response) {
        const origin = backendRequest(request)
        for (const header of ['If-None-Match', 'If-Modified-Since', 'Range', 'If-Range'])
            origin.headers.delete(header)
        origin.headers.set('If-Match', etag)
        const fetched = await fetch(origin, originOptions)
        if (fetched.status !== 200) return privateResponse(fetched)
        if (
            fetched.headers.get('ETag') !== etag ||
            Number(fetched.headers.get('Content-Length')) !== length ||
            fetched.headers.get('Content-Type') !== mime[match[2]]
        ) {
            await fetched.body?.cancel()
            return new Response(null, {
                status: 503,
                headers: { 'Cache-Control': 'private, no-store' },
            })
        }
        // Originals and renditions are copied lazily. SHA-256 verifies the transfer;
        // objects are immutable and private, and no credentials enter the browser.
        const copy = fetched.clone()
        later(
            context,
            env.MEDIA_BUCKET.put(key, copy.body, {
                sha256: match[1],
                httpMetadata: { contentType: mime[match[2]], cacheControl: 'private, no-store' },
                customMetadata: { sha256: match[1] },
                onlyIf: { etagDoesNotMatch: '*' },
            }),
        )
        headers.set('X-Lycoris-Media-Source', 'origin')
        response = new Response(fetched.body, { headers })
    }
    if (cache) {
        const copy = response.clone()
        // Only the internal content cache is shared. Browser responses stay no-store,
        // and the internal cache path is never served without the permission check.
        const internal = new Response(copy.body, {
            headers: {
                'Content-Type': mime[match[2]],
                'Content-Length': String(length),
                ETag: etag,
                'Cache-Control': 'public, max-age=86400',
            },
        })
        later(context, cache.put(cacheKey, internal))
    }
    return response
}

export default {
    async fetch(request, env, context) {
        const incoming = new URL(request.url)
        if (incoming.pathname === '/tiles/osm' || incoming.pathname.startsWith('/tiles/osm/'))
            return osmTile(request)
        if (incoming.pathname.startsWith('/__lycoris_media_cache/'))
            return new Response(null, { status: 404 })
        const isBackend = ['/api', '/uploads', '/health'].some(
            (prefix) => incoming.pathname === prefix || incoming.pathname.startsWith(`${prefix}/`),
        )
        if (!isBackend) return env.ASSETS.fetch(request)

        // Preserve Origin, cookies, methods and streaming bodies. The Rust server
        // validates the real browser Origin; redirects must not forward credentials.
        try {
            if (
                env.MEDIA_BUCKET &&
                ['GET', 'HEAD'].includes(request.method) &&
                MEDIA_PATH.test(incoming.pathname)
            ) {
                return await controlledImage(request, env, context)
            }
            return privateResponse(await fetch(backendRequest(request), originOptions))
        } catch {
            return Response.json(
                { message: 'Service temporarily unavailable' },
                { status: 503, headers: { 'Cache-Control': 'private, no-store' } },
            )
        }
    },
}
