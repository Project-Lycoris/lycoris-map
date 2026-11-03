import assert from 'node:assert/strict'
import { test } from 'node:test'
import worker from './_worker.js'

test('OSM requests use a fixed upstream, preserve attribution headers and never leak session credentials', async (t) => {
    t.mock.method(globalThis, 'fetch', async (request, options) => {
        assert.equal(request.url, 'https://tile.openstreetmap.org/14/13720/6693.png')
        assert.equal(request.headers.get('Cookie'), null)
        assert.equal(request.headers.get('Authorization'), null)
        assert.equal(request.headers.get('Cache-Control'), null)
        assert.equal(request.headers.get('Pragma'), null)
        assert.equal(request.headers.get('Referer'), 'https://lycoris-map.com/')
        assert.match(request.headers.get('User-Agent'), /^LycorisMaps\/1\.0 /)
        assert.deepEqual(options, { redirect: 'manual', cf: { cacheEverything: true } })
        return new Response('tile', {
            headers: {
                'Content-Type': 'image/png',
                'Cache-Control': 'max-age=24845',
                ETag: '"tile-v1"',
                Age: '7',
                'CF-Cache-Status': 'HIT',
                'Set-Cookie': 'provider=never-forward',
            },
        })
    })
    const result = await worker.fetch(
        new Request('https://lycoris-map.com/tiles/osm/14/13720/6693.png', {
            headers: {
                Cookie: 'session=secret',
                Authorization: 'Bearer secret',
                // The page URL identifies the viewed place; only the origin may reach OSM.
                Referer: 'https://lycoris-map.com/maps?markerId=42&lat=31.2304&lng=121.4737',
                'Cache-Control': 'no-cache',
                Pragma: 'no-cache',
            },
        }),
        {},
    )
    assert.equal(await result.text(), 'tile')
    assert.equal(result.headers.get('Cache-Control'), 'max-age=24845')
    assert.equal(result.headers.get('ETag'), '"tile-v1"')
    assert.equal(result.headers.get('Age'), '7')
    assert.equal(result.headers.get('Set-Cookie'), null)
    assert.equal(result.headers.get('X-Lycoris-Tile-Cache'), 'HIT')
})

test('OSM rejects unsupported coordinates, cache-busting queries, methods and arbitrary proxy targets', async (t) => {
    const fetch = t.mock.method(globalThis, 'fetch', () => {
        throw Error('must not fetch')
    })
    for (const path of [
        '/tiles/osm',
        '/tiles/osm/20/0/0.png',
        '/tiles/osm/0/1/0.png',
        '/tiles/osm/14/0/16384.png',
        '/tiles/osm/01/0/0.png',
        '/tiles/osm/1/-1/0.png',
        '/tiles/osm/0/0/0.png?url=https://example.com',
        '/tiles/osm/https://example.com',
    ]) {
        const response = await worker.fetch(new Request(`https://lycoris-map.com${path}`), {})
        assert.equal(response.status, 404, path)
        assert.equal(response.headers.get('Cache-Control'), 'no-store')
    }
    const post = await worker.fetch(
        new Request('https://lycoris-map.com/tiles/osm/0/0/0.png', { method: 'POST' }),
        {},
    )
    assert.equal(post.status, 405)
    assert.equal(post.headers.get('Allow'), 'GET, HEAD')
    assert.equal(fetch.mock.callCount(), 0)
})

test('OSM preserves conditional and HEAD responses for browser caching', async (t) => {
    t.mock.method(globalThis, 'fetch', async (request) => {
        if (request.method === 'HEAD')
            return new Response(null, {
                headers: { 'Content-Type': 'image/png', 'Content-Length': '37809' },
            })
        assert.equal(request.headers.get('If-None-Match'), '"tile-v1"')
        assert.equal(request.headers.get('If-Modified-Since'), 'Sun, 20 Sep 2026 00:00:00 GMT')
        return new Response(null, {
            status: 304,
            headers: { ETag: '"tile-v1"', 'Cache-Control': 'max-age=3600' },
        })
    })
    const url = 'https://lycoris-map.com/tiles/osm/0/0/0.png'
    const head = await worker.fetch(new Request(url, { method: 'HEAD' }), {})
    assert.equal(head.headers.get('Content-Length'), '37809')
    assert.equal(await head.text(), '')
    const conditional = await worker.fetch(
        new Request(url, {
            headers: {
                'If-None-Match': '"tile-v1"',
                'If-Modified-Since': 'Sun, 20 Sep 2026 00:00:00 GMT',
            },
        }),
        {},
    )
    assert.equal(conditional.status, 304)
    assert.equal(conditional.headers.get('Cache-Control'), 'max-age=3600')
})

test('OSM upstream outages, redirects and error images never become successful cached map tiles', async (t) => {
    let response
    t.mock.method(globalThis, 'fetch', async () => {
        if (!response) throw Error('timeout')
        return response
    })
    for (const upstream of [
        undefined,
        new Response('denied', { status: 403, headers: { 'Content-Type': 'image/png' } }),
        new Response('blocked', {
            headers: { 'Content-Type': 'image/png', 'x-blocked': 'policy' },
        }),
        new Response('error', { headers: { 'Content-Type': 'text/html' } }),
        new Response(null, { status: 302, headers: { Location: 'https://example.com' } }),
    ]) {
        response = upstream
        const result = await worker.fetch(
            new Request('https://lycoris-map.com/tiles/osm/0/0/0.png'),
            {},
        )
        assert.equal(result.status, 502)
        assert.equal(result.headers.get('Cache-Control'), 'no-store')
    }
})

test('static pages and admin deep links stay on Pages', async () => {
    for (const path of ['/', '/admin/review', '/assets/app.js', '/api-not-a-route']) {
        const request = new Request(`https://lycoris-map.com${path}`)
        const response = await worker.fetch(request, {
            ASSETS: {
                fetch: (received) => {
                    assert.equal(received, request)
                    return new Response('asset')
                },
            },
        })
        assert.equal(await response.text(), 'asset')
    }
})

test('API writes preserve authentication, Origin and body without caching or following redirects', async (t) => {
    t.mock.method(globalThis, 'fetch', async (request, options) => {
        assert.equal(request.url, 'https://api.lycoris-map.com/api/markers?lang=en')
        assert.equal(request.method, 'POST')
        assert.equal(request.headers.get('Origin'), 'https://lycoris-map.com')
        assert.equal(request.headers.get('Cookie'), 'LYCORIS_SESSION=test')
        assert.equal(request.headers.get('X-Real-IP'), '203.0.113.9')
        assert.equal(request.headers.get('X-Forwarded-For'), '203.0.113.9')
        assert.equal(request.headers.get('Forwarded'), null)
        assert.equal(await request.text(), '{"name":"A"}')
        assert.equal(options.redirect, 'manual')
        assert.equal(options.cf.cacheTtl, 0)
        return new Response('created', {
            status: 201,
            headers: { 'Set-Cookie': 'LYCORIS_SESSION=next; Secure; HttpOnly; Path=/' },
        })
    })
    const response = await worker.fetch(
        new Request('https://lycoris-map.com/api/markers?lang=en', {
            method: 'POST',
            headers: {
                Origin: 'https://lycoris-map.com',
                Cookie: 'LYCORIS_SESSION=test',
                'CF-Connecting-IP': '203.0.113.9',
                'X-Real-IP': '198.51.100.1',
                'X-Forwarded-For': '198.51.100.2',
                Forwarded: 'for=198.51.100.3',
            },
            body: '{"name":"A"}',
        }),
        {},
    )
    assert.equal(response.status, 201)
    assert.equal(response.headers.get('Cache-Control'), 'private, no-store')
    assert.match(response.headers.get('Set-Cookie'), /LYCORIS_SESSION=next/)
})

test('uploads retain range headers; API failures do not fall through to SPA HTML', async (t) => {
    t.mock.method(globalThis, 'fetch', async (request) => {
        assert.equal(request.url, 'https://api.lycoris-map.com/uploads/markers/a.jpg')
        assert.equal(request.headers.get('Range'), 'bytes=0-99')
        throw new Error('unreachable')
    })
    const response = await worker.fetch(
        new Request('https://lycoris-map.com/uploads/markers/a.jpg', {
            headers: { Range: 'bytes=0-99' },
        }),
        {},
    )
    assert.equal(response.status, 503)
    assert.deepEqual(await response.json(), { message: 'Service temporarily unavailable' })
})

async function mediaFixture(t) {
    const bytes = new TextEncoder().encode('synthetic image bytes')
    const digest = [...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))]
        .map((v) => v.toString(16).padStart(2, '0'))
        .join('')
    const etag = `"${digest}"`
    const key = `media/v1/${digest}.jpg`
    const objects = new Map()
    const edges = new Map()
    const calls = []
    const tasks = []
    const state = {
        status: 200,
        r2Failure: false,
        edgeFailure: false,
        getStatus: 200,
        getEtag: etag,
        ownerOnly: false,
    }
    const object = { bytes, customMetadata: { sha256: digest } }
    const cache = {
        async match(request) {
            calls.push('cache')
            if (state.edgeFailure) throw Error('cache unavailable')
            return edges.get(request.url)?.clone()
        },
        async put(request, response) {
            if (state.edgeFailure) throw Error('cache unavailable')
            assert.equal(response.headers.get('Set-Cookie'), null)
            edges.set(request.url, new Response(await response.arrayBuffer(), response))
        },
    }
    const oldCaches = globalThis.caches
    globalThis.caches = { default: cache }
    t.after(() => {
        if (oldCaches) globalThis.caches = oldCaches
        else delete globalThis.caches
    })
    t.mock.method(globalThis, 'fetch', async (request, options) => {
        assert.equal(new URL(request.url).origin, 'https://api.lycoris-map.com')
        assert.equal(options.cf.cacheTtl, 0)
        assert.equal(options.redirect, 'manual')
        calls.push(request.method)
        const status =
            state.ownerOnly && request.headers.get('Cookie') !== 'owner=yes' ? 404 : state.status
        if (status !== 200) return new Response(null, { status })
        const headers = {
            'Content-Type': 'image/jpeg',
            'Content-Length': String(bytes.length),
            'x-lycoris-media-key': key,
            ETag: request.method === 'HEAD' ? etag : state.getEtag,
            'Cache-Control': 'no-store',
            'Set-Cookie': 'origin-cookie=refreshed; HttpOnly',
        }
        if (request.method === 'HEAD') {
            assert.equal(request.headers.get('If-None-Match'), null)
            assert.equal(request.headers.get('Range'), null)
            return new Response(null, { headers })
        }
        assert.equal(request.headers.get('If-Match'), etag)
        assert.equal(request.headers.get('Range'), null)
        return new Response(state.getStatus === 200 ? bytes : null, {
            headers,
            status: state.getStatus,
        })
    })
    const env = {
        MEDIA_BUCKET: {
            async get(k) {
                calls.push('r2')
                if (state.r2Failure) throw Error('R2 unavailable')
                const value = objects.get(k)
                return value
                    ? {
                          body: new Response(value.bytes).body,
                          size: value.bytes.length,
                          customMetadata: value.customMetadata,
                      }
                    : null
            },
            async put(k, stream, options) {
                calls.push('put')
                if (state.r2Failure) throw Error('R2 unavailable')
                assert.equal(k, key)
                assert.equal(options.sha256, digest)
                assert.equal(options.onlyIf.etagDoesNotMatch, '*')
                const data = new Uint8Array(await new Response(stream).arrayBuffer())
                assert.deepEqual(data, bytes)
                objects.set(k, { bytes: data, customMetadata: options.customMetadata })
            },
        },
    }
    const context = {
        waitUntil(p) {
            tasks.push(p)
        },
    }
    const fetch = async (options = {}) => {
        const response = await worker.fetch(
            new Request('https://lycoris-map.com/uploads/markers/test.jpg?variant=thumb', options),
            env,
            context,
        )
        const body = new Uint8Array(await response.arrayBuffer())
        await Promise.all(tasks.splice(0))
        return { response, body }
    }
    return { state, calls, bytes, key, etag, objects, object, edges, fetch }
}

test('cold images populate private R2 and edge cache; warm images still authorize before returning bytes', async (t) => {
    const f = await mediaFixture(t)
    const cold = await f.fetch()
    assert.deepEqual(cold.body, f.bytes)
    assert.equal(cold.response.headers.get('X-Lycoris-Media-Source'), 'origin')
    assert.equal(cold.response.headers.get('Cache-Control'), 'private, no-store')
    assert.equal(f.objects.size, 1)
    assert.equal(f.edges.size, 1)
    f.calls.length = 0
    const warm = await f.fetch()
    assert.deepEqual(f.calls, ['HEAD', 'cache'])
    assert.deepEqual(warm.body, f.bytes)
    assert.equal(warm.response.headers.get('X-Lycoris-Media-Source'), 'edge')
    assert.equal(warm.response.headers.get('Set-Cookie'), 'origin-cookie=refreshed; HttpOnly')
    assert.equal(warm.response.headers.has('x-lycoris-media-key'), false)
    f.edges.clear()
    f.calls.length = 0
    const regional = await f.fetch()
    assert.deepEqual(f.calls, ['HEAD', 'cache', 'r2'])
    assert.equal(regional.response.headers.get('X-Lycoris-Media-Source'), 'r2')
})

test('deactivation, logout and origin outages deny access even with cached bytes and matching ETag', async (t) => {
    const f = await mediaFixture(t)
    await f.fetch()
    for (const status of [404, 401, 503]) {
        f.calls.length = 0
        f.state.status = status
        const result = await f.fetch({ headers: { 'If-None-Match': f.etag } })
        assert.equal(result.response.status, status)
        assert.equal(result.body.length, 0)
        assert.deepEqual(f.calls, ['HEAD'])
    }
    f.state.status = 200
    f.state.ownerOnly = true
    assert.equal((await f.fetch({ headers: { Cookie: 'owner=yes' } })).response.status, 200)
    f.calls.length = 0
    assert.equal((await f.fetch()).response.status, 404)
    assert.deepEqual(f.calls, ['HEAD'])
})

test('HEAD and conditional reads authorize without downloading objects', async (t) => {
    const f = await mediaFixture(t)
    assert.equal((await f.fetch({ method: 'HEAD' })).body.length, 0)
    assert.equal(
        (await f.fetch({ headers: { 'If-None-Match': `W/${f.etag}` } })).response.status,
        304,
    )
    assert.equal((await f.fetch({ headers: { 'If-Match': '"old"' } })).response.status, 412)
    assert.deepEqual(f.calls, ['HEAD', 'HEAD', 'HEAD'])
})

test('R2 and cache failures fall back to authorized origin; Range never poisons complete-object cache', async (t) => {
    const f = await mediaFixture(t)
    f.state.r2Failure = true
    f.state.edgeFailure = true
    const result = await f.fetch({ headers: { Range: 'bytes=0-2' } })
    assert.equal(result.response.status, 200)
    assert.deepEqual(result.body, f.bytes)
    assert.equal(result.response.headers.has('Content-Range'), false)
})

test('a changed source or revoked permission during cache fill never gets stored under an old key', async (t) => {
    const f = await mediaFixture(t)
    f.state.getEtag = '"changed"'
    assert.equal((await f.fetch()).response.status, 503)
    assert.equal(f.objects.size, 0)
    assert.equal(f.edges.size, 0)
    f.state.getEtag = f.etag
    f.state.getStatus = 404
    assert.equal((await f.fetch()).response.status, 404)
    assert.equal(f.objects.size, 0)
})

test('untrusted R2 metadata does not override the origin identity', async (t) => {
    const f = await mediaFixture(t)
    f.objects.set(f.key, { ...f.object, customMetadata: { sha256: 'invalid' } })
    const result = await f.fetch()
    assert.equal(result.response.headers.get('X-Lycoris-Media-Source'), 'origin')
    assert.deepEqual(result.body, f.bytes)
})

test('internal content cache cannot be read directly', async () => {
    const response = await worker.fetch(
        new Request('https://lycoris-map.com/__lycoris_media_cache/media/v1/test.jpg'),
        {},
    )
    assert.equal(response.status, 404)
})
