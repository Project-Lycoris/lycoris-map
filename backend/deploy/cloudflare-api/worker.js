// Preserve the API address used by the Web edge and installed native apps.
// Shanghai is the only database/write authority; never cache account responses.
const ORIGIN = 'https://lycoris-map.cn'

export default {
    async fetch(request) {
        const incoming = new URL(request.url)
        if (!/^\/(api|uploads|health)(\/|$)/.test(incoming.pathname)) {
            return new Response(null, { status: 404 })
        }
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
            return new Response('Service temporarily unavailable', {
                status: 502,
                headers: { 'Cache-Control': 'no-store', 'Content-Type': 'text/plain; charset=utf-8' },
            })
        }
    },
}
