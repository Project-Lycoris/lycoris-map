import { test } from 'node:test'
import assert from 'node:assert/strict'
import worker from './worker.js'

test('forwards writes and credentials only to Shanghai without caching or following redirects', async () => {
    const original = globalThis.fetch
    globalThis.fetch = async (request, options) => {
        assert.equal(request.url, 'https://lycoris-map.cn/api/markers?lang=zh')
        assert.equal(request.method, 'POST')
        assert.equal(await request.text(), '{"test":true}')
        assert.equal(request.headers.get('Origin'), 'https://lycoris-map.com')
        assert.equal(request.headers.get('Authorization'), 'Bearer synthetic')
        assert.equal(request.headers.get('Cookie'), 'LYCORIS_SESSION=synthetic')
        assert.equal(request.headers.get('X-Forwarded-For'), '203.0.113.9')
        assert.equal(request.headers.get('Forwarded'), null)
        assert.equal(options.redirect, 'manual')
        assert.equal(options.cf.cacheTtl, 0)
        return new Response('ok', { headers: { 'Set-Cookie': 'LYCORIS_SESSION=new; Secure; HttpOnly' } })
    }
    try {
        const response = await worker.fetch(new Request('https://api.lycoris-map.com/api/markers?lang=zh', {
            method: 'POST', body: '{"test":true}', headers: {
                Origin: 'https://lycoris-map.com', Authorization: 'Bearer synthetic',
                Cookie: 'LYCORIS_SESSION=synthetic', 'CF-Connecting-IP': '203.0.113.9',
                'X-Forwarded-For': 'spoofed', Forwarded: 'for=spoofed',
            },
        }))
        assert.equal(response.headers.get('Cache-Control'), 'private, no-store')
        assert.equal(response.headers.get('Set-Cookie'), 'LYCORIS_SESSION=new; Secure; HttpOnly')
    } finally { globalThis.fetch = original }
})

test('retains authorization metadata required by the existing private R2 delivery path', async () => {
    const original = globalThis.fetch
    globalThis.fetch = async (request) => {
        assert.equal(request.method, 'HEAD')
        return new Response(null, { headers: { 'X-Lycoris-Media-Key': 'media/v1/synthetic.jpg' } })
    }
    try {
        const response = await worker.fetch(new Request('https://api.lycoris-map.com/uploads/markers/photo.jpg', { method: 'HEAD' }))
        assert.equal(response.headers.get('X-Lycoris-Media-Key'), 'media/v1/synthetic.jpg')
    } finally { globalThis.fetch = original }
})

test('fails closed for non-API paths and an unavailable Shanghai origin', async () => {
    const original = globalThis.fetch
    globalThis.fetch = async () => { throw new Error('offline') }
    try {
        assert.equal((await worker.fetch(new Request('https://api.lycoris-map.com/'))).status, 404)
        assert.equal((await worker.fetch(new Request('https://api.lycoris-map.com/api/users/me'))).status, 502)
    } finally { globalThis.fetch = original }
})
