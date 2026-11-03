// Preserve the API address used by the Web edge and installed native apps.
// Shanghai is the only database/write authority; never cache account responses.
const ORIGIN = 'https://lycoris-map.cn'
// Shanghai trusts X-Forwarded-For only with this secret. Cloudflare's egress
// addresses are shared by every Worker, so the address alone proves nothing.
const EDGE_SECRET_HEADER = 'X-Lycoris-Edge-Secret'

function unavailable() {
    return new Response('Service temporarily unavailable', {
        status: 502,
        headers: { 'Cache-Control': 'no-store', 'Content-Type': 'text/plain; charset=utf-8' },
    })
}

export default {
    async fetch(request, env = {}) {
        const incoming = new URL(request.url)
        if (!/^\/(api|uploads|health)(\/|$)/.test(incoming.pathname)) {
            return new Response(null, { status: 404 })
        }
        // Without the secret every user would share one rate-limit identity: fail closed.
        const secret = typeof env.EDGE_SECRET === 'string' ? env.EDGE_SECRET : ''
        if (secret.length < 32) return unavailable()
        const target = new URL(ORIGIN)
        target.pathname = incoming.pathname
        target.search = incoming.search
        const forwarded = new Request(target, request)
        forwarded.headers.delete('Host')
        forwarded.headers.delete('Forwarded')
        forwarded.headers.set('X-Forwarded-Host', incoming.host)
        forwarded.headers.set('X-Forwarded-Proto', 'https')
        // Replace, rather than append to, a client-supplied address chain.
        forwarded.headers.set('X-Forwarded-For', request.headers.get('CF-Connecting-IP') || '')
        forwarded.headers.set(EDGE_SECRET_HEADER, secret)
        try {
            const response = await fetch(forwarded, {
                redirect: 'manual',
                signal: AbortSignal.timeout(60000),
                cf: { cacheTtl: 0, cacheEverything: false },
            })
            const outgoing = new Response(response.body, response)
            outgoing.headers.set('Cache-Control', 'private, no-store')
            outgoing.headers.set('X-Lycoris-Gateway', 'shanghai-edge')
            return outgoing
        } catch {
            return unavailable()
        }
    },
}
