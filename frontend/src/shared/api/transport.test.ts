import { afterEach, describe, expect, it, vi } from 'vitest'
import { ApiError } from './ApiError'
import { request, requestBlob } from './transport'

type FetchArgs = Parameters<typeof fetch>

function jsonResponse(body: unknown, init: ResponseInit = {}): Response {
    return new Response(JSON.stringify(body), {
        status: 200,
        headers: { 'content-type': 'application/json' },
        ...init,
    })
}

function stubFetch(response: Response | ((...args: FetchArgs) => Promise<Response>)) {
    const mock = vi.fn<(...args: FetchArgs) => Promise<Response>>(
        typeof response === 'function' ? response : async () => response,
    )
    vi.stubGlobal('fetch', mock)
    return mock
}

afterEach(() => {
    vi.unstubAllGlobals()
})

describe('request', () => {
    it('sends credentials include for cookie sessions', async () => {
        const mock = stubFetch(jsonResponse({ ok: true }))
        await request('/api/me')
        expect(mock).toHaveBeenCalledTimes(1)
        expect(mock.mock.calls[0]?.[1]?.credentials).toBe('include')
    })

    it('builds query strings and drops empty values', async () => {
        const mock = stubFetch(jsonResponse([]))
        await request('/api/markers/viewport', {
            query: { minLat: 1, maxLat: 2, categories: undefined, lang: 'zh' },
        })
        expect(mock.mock.calls[0]?.[0]).toBe('/api/markers/viewport?minLat=1&maxLat=2&lang=zh')
    })

    it('propagates a caller cancellation through the deadline signal', async () => {
        const mock = stubFetch(
            (_input, init) =>
                new Promise<Response>((_resolve, reject) => {
                    init?.signal?.addEventListener('abort', () =>
                        reject(new DOMException('aborted', 'AbortError')),
                    )
                }),
        )
        const controller = new AbortController()
        const pending = request('/api/me', { signal: controller.signal })
        const forwarded = mock.mock.calls[0]?.[1]?.signal
        // The forwarded signal is the deadline wrapper, not the caller's
        // identity; cancelling the caller must still abort the live request.
        expect(forwarded).toBeInstanceOf(AbortSignal)
        expect(forwarded?.aborted).toBe(false)
        controller.abort()
        expect(forwarded?.aborted).toBe(true)
        await expect(pending).rejects.toBeInstanceOf(DOMException)
    })

    it('serializes a JSON body with the JSON content type', async () => {
        const mock = stubFetch(jsonResponse({ code: 0, message: 'ok', data: null }))
        await request('/api/login', { method: 'POST', json: { username: 'u', password: 'p' } })
        const init = mock.mock.calls[0]?.[1]
        expect(init?.body).toBe(JSON.stringify({ username: 'u', password: 'p' }))
        expect((init?.headers as Record<string, string>)['Content-Type']).toBe('application/json')
    })

    it('parses a bare JSON array (marker reads have no envelope)', async () => {
        stubFetch(jsonResponse([{ id: 1 }, { id: 2 }]))
        await expect(request('/api/markers/nearby')).resolves.toEqual([{ id: 1 }, { id: 2 }])
    })

    it('resolves undefined for a 200 with an empty body', async () => {
        stubFetch(new Response('', { status: 200 }))
        await expect(
            request('/api/markers/1/favorite', { method: 'POST' }),
        ).resolves.toBeUndefined()
    })

    it('returns plain text when the response is not JSON', async () => {
        stubFetch(
            new Response('缺少 q 参数', {
                status: 400,
                headers: { 'content-type': 'text/plain; charset=utf-8' },
            }),
        )
        await expect(request('/api/markers/search')).rejects.toMatchObject({
            status: 400,
            message: '缺少 q 参数',
        })
    })

    it('keeps the Auth envelope business code on failure', async () => {
        stubFetch(
            jsonResponse(
                { code: 4001, message: 'Invalid username or password', data: null },
                { status: 401, headers: { 'content-type': 'application/json' } },
            ),
        )
        const error = await request('/api/login', { method: 'POST', json: {} }).catch(
            (value: unknown) => value,
        )
        expect(error).toBeInstanceOf(ApiError)
        const apiError = error as ApiError
        expect(apiError.status).toBe(401)
        expect(apiError.code).toBe(4001)
        expect(apiError.message).toBe('Invalid username or password')
    })

    it('stores the raw response body on a 429 so retry hints survive transport', async () => {
        stubFetch(
            jsonResponse(
                {
                    code: 42933,
                    message: 'Too many sign-in attempts. Try again later.',
                    data: { retryAfterSeconds: 7 },
                },
                { status: 429, headers: { 'content-type': 'application/json' } },
            ),
        )
        const error = (await request('/api/login', {
            method: 'POST',
            json: { username: 'synthetic', password: 'synthetic-password' },
        }).catch((value: unknown) => value)) as ApiError
        expect(error).toBeInstanceOf(ApiError)
        expect(error.status).toBe(429)
        expect(error.code).toBe(42933)
        expect(JSON.parse(error.body ?? '{}')).toMatchObject({ data: { retryAfterSeconds: 7 } })
        // Only the response is kept; the request credentials must never leak in.
        expect(error.body).not.toContain('synthetic-password')
    })

    it('reads the fixed 401 security body', async () => {
        stubFetch(
            jsonResponse(
                { message: 'Spring Security Error' },
                { status: 401, headers: { 'content-type': 'application/json' } },
            ),
        )
        await expect(request('/api/me')).rejects.toMatchObject({
            status: 401,
            message: 'Spring Security Error',
        })
    })

    it('captures the X-Request-ID header for diagnostics', async () => {
        stubFetch(
            jsonResponse(
                { message: 'sensitive' },
                {
                    status: 404,
                    headers: { 'content-type': 'application/json', 'x-request-id': 'req-42' },
                },
            ),
        )
        const error = (await request('/api/markers/9').catch((value: unknown) => value)) as ApiError
        expect(error.requestId).toBe('req-42')
    })

    it('does not treat every 401 as a global logout', async () => {
        stubFetch(new Response('', { status: 401, headers: { 'x-request-id': 'req-9' } }))
        const error = (await request('/api/markers/me/created').catch(
            (value: unknown) => value,
        )) as ApiError
        // Transport only reports; no module-level session state is touched.
        expect(error.status).toBe(401)
        expect(error.message).toBe('HTTP 401')
    })

    it('rethrows AbortError untouched', async () => {
        const controller = new AbortController()
        stubFetch(() => {
            controller.abort()
            return Promise.reject(new DOMException('aborted', 'AbortError'))
        })
        await expect(request('/api/me', { signal: controller.signal })).rejects.toBeInstanceOf(
            DOMException,
        )
    })

    it('maps a network failure to status 0', async () => {
        stubFetch(() => Promise.reject(new TypeError('Failed to fetch')))
        await expect(request('/api/me')).rejects.toMatchObject({ status: 0 })
    })
})

describe('requestBlob', () => {
    /** A tiny valid PNG signature block; enough to assert byte fidelity. */
    const pngBytes = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x01])

    function pngResponse(init: ResponseInit = {}): Response {
        return new Response(pngBytes, {
            status: 200,
            headers: { 'content-type': 'image/png' },
            ...init,
        })
    }

    it('returns the raw image bytes as a Blob for image/png', async () => {
        stubFetch(pngResponse())
        const result = await requestBlob('/api/me/avatar')

        expect(result).toBeInstanceOf(Blob)
        expect(result.type).toBe('image/png')
        const bytes = new Uint8Array(await result.arrayBuffer())
        expect([...bytes]).toEqual([...pngBytes])
        // Text decoding would have corrupted these bytes.
        expect(bytes[0]).toBe(0x89)
    })

    it('sends credentials include and requests an image', async () => {
        const mock = stubFetch(pngResponse())
        await requestBlob('/api/users/uuid/avatar')
        const init = mock.mock.calls[0]?.[1]
        expect(init?.credentials).toBe('include')
        expect((init?.headers as Record<string, string>).Accept).toBe('image/*')
        expect(mock.mock.calls[0]?.[0]).toBe('/api/users/uuid/avatar')
    })

    it('throws ApiError with status 404 for an empty body', async () => {
        stubFetch(new Response('', { status: 404, headers: { 'x-request-id': 'req-404' } }))
        const error = (await requestBlob('/api/me/avatar').catch(
            (value: unknown) => value,
        )) as ApiError

        expect(error).toBeInstanceOf(ApiError)
        expect(error.status).toBe(404)
        expect(error.message).toBe('HTTP 404')
        expect(error.requestId).toBe('req-404')
        expect(error.body).toBeUndefined()
    })

    it('parses a JSON error body on failure instead of returning it as bytes', async () => {
        stubFetch(
            jsonResponse(
                { message: '用户不存在' },
                { status: 404, headers: { 'content-type': 'application/json' } },
            ),
        )
        await expect(requestBlob('/api/users/x/avatar')).rejects.toMatchObject({
            status: 404,
            message: '用户不存在',
        })
    })

    it('stores the raw JSON response body for blob failures too', async () => {
        stubFetch(
            jsonResponse(
                {
                    code: 42933,
                    message: 'Too many sign-in attempts. Try again later.',
                    data: { retryAfterSeconds: 7 },
                },
                { status: 429, headers: { 'content-type': 'application/json' } },
            ),
        )
        const error = (await requestBlob('/api/me/avatar').catch(
            (value: unknown) => value,
        )) as ApiError
        expect(JSON.parse(error.body ?? '{}')).toMatchObject({ data: { retryAfterSeconds: 7 } })
    })

    it('always issues a plain GET with no request body', async () => {
        const mock = stubFetch(pngResponse())
        await requestBlob('/api/me/avatar')
        const init = mock.mock.calls[0]?.[1]
        expect(init?.method).toBe('GET')
        expect(init).not.toHaveProperty('body')
        expect((init?.headers as Record<string, string>)['Content-Type']).toBeUndefined()
    })

    it('forwards query, headers, a cancellable signal and baseUrl', async () => {
        const mock = stubFetch(
            (_input, init) =>
                new Promise<Response>((_resolve, reject) => {
                    init?.signal?.addEventListener('abort', () =>
                        reject(new DOMException('aborted', 'AbortError')),
                    )
                }),
        )
        const controller = new AbortController()
        const pending = requestBlob('/api/users/uuid/avatar', {
            query: { v: 2, skip: undefined },
            headers: { 'X-App-Language': 'zh' },
            signal: controller.signal,
            baseUrl: 'https://api.example.test',
        })
        const init = mock.mock.calls[0]?.[1]
        expect(mock.mock.calls[0]?.[0]).toBe('https://api.example.test/api/users/uuid/avatar?v=2')
        expect((init?.headers as Record<string, string>)['X-App-Language']).toBe('zh')
        expect(init?.signal).toBeInstanceOf(AbortSignal)
        controller.abort()
        expect(init?.signal?.aborted).toBe(true)
        await expect(pending).rejects.toBeInstanceOf(DOMException)
    })

    it('rethrows AbortError untouched', async () => {
        const controller = new AbortController()
        stubFetch(() => Promise.reject(new DOMException('aborted', 'AbortError')))
        await expect(
            requestBlob('/api/me/avatar', { signal: controller.signal }),
        ).rejects.toBeInstanceOf(DOMException)
    })

    it('keeps plain JSON requests unaffected', async () => {
        stubFetch(jsonResponse({ ok: true }))
        await expect(request('/api/me')).resolves.toEqual({ ok: true })
    })
})

describe('bounded deadline', () => {
    function hangingFetch(): ReturnType<typeof stubFetch> {
        // Real-ish fetch: never resolves, but rejects when its signal aborts.
        return stubFetch(
            (_input, init) =>
                new Promise<Response>((_resolve, reject) => {
                    const signal = init?.signal
                    if (!signal) return
                    if (signal.aborted) {
                        reject(new DOMException('aborted', 'AbortError'))
                        return
                    }
                    signal.addEventListener('abort', () =>
                        reject(new DOMException('aborted', 'AbortError')),
                    )
                }),
        )
    }
    function hangingBodyResponse(body: 'text' | 'blob'): Response {
        const response = new Response('x', {
            status: 200,
            headers: { 'content-type': 'application/json' },
        })
        if (body === 'text')
            vi.spyOn(response, 'text').mockImplementation(() => new Promise(() => {}))
        else vi.spyOn(response, 'blob').mockImplementation(() => new Promise(() => {}))
        return response
    }

    it('turns a hung fetch into a network timeout', async () => {
        hangingFetch()
        const error = await request('/api/me', { timeoutMs: 30 }).catch((v: unknown) => v)
        expect(error).toBeInstanceOf(ApiError)
        expect((error as ApiError).status).toBe(0)
        expect((error as ApiError).message).toContain('timed out')
    })

    it('turns a hung response body read into a network timeout', async () => {
        stubFetch(() => Promise.resolve(hangingBodyResponse('text')))
        const error = await request('/api/me', { timeoutMs: 30 }).catch((v: unknown) => v)
        expect(error).toBeInstanceOf(ApiError)
        expect((error as ApiError).status).toBe(0)
        expect((error as ApiError).message).toContain('timed out')
    })

    it('bounds a hung blob body too', async () => {
        stubFetch(() => Promise.resolve(hangingBodyResponse('blob')))
        const error = await requestBlob('/api/me/avatar', { timeoutMs: 30 }).catch(
            (v: unknown) => v,
        )
        expect(error).toBeInstanceOf(ApiError)
        expect((error as ApiError).message).toContain('timed out')
    })

    it('keeps a caller cancellation as AbortError, not a network error', async () => {
        hangingFetch()
        const controller = new AbortController()
        const pending = request('/api/me', { signal: controller.signal, timeoutMs: 5_000 })
        controller.abort()
        await expect(pending).rejects.toBeInstanceOf(DOMException)
        await expect(pending.catch((v: unknown) => v)).resolves.not.toBeInstanceOf(ApiError)
    })

    it('clears the deadline timer after a success so later work is unaffected', async () => {
        stubFetch(async () => jsonResponse({ ok: true }))
        const original = globalThis.clearTimeout
        const clear = vi.fn((id?: number) => original(id))
        vi.stubGlobal('clearTimeout', clear)
        await request('/api/me')
        expect(clear).toHaveBeenCalled()
        // A subsequent request still runs normally.
        await expect(request('/api/me')).resolves.toEqual({ ok: true })
    })

    it('does not interfere with the next request after a timeout', async () => {
        hangingFetch()
        await expect(request('/api/me', { timeoutMs: 20 })).rejects.toMatchObject({ status: 0 })
        stubFetch(jsonResponse({ ok: true }))
        await expect(request('/api/me', { timeoutMs: 1_000 })).resolves.toEqual({ ok: true })
    })

    it('does not dispatch a write when the caller signal is already aborted', async () => {
        const dispatched = vi.fn()
        const fetcher = vi.fn((_input: RequestInfo | URL, init?: RequestInit) => {
            // A pre-aborted signal must reach fetch as aborted; a real fetch
            // would reject without sending. Record any attempted dispatch.
            if (!init?.signal?.aborted) dispatched()
            return new Promise<Response>((_resolve, reject) => {
                init?.signal?.addEventListener('abort', () =>
                    reject(new DOMException('aborted', 'AbortError')),
                )
            })
        })
        vi.stubGlobal('fetch', fetcher)
        const controller = new AbortController()
        controller.abort()
        const error = await request('/api/markers/1/favorite', {
            method: 'POST',
            signal: controller.signal,
        }).catch((value: unknown) => value)
        expect(error).toBeInstanceOf(DOMException)
        expect((error as DOMException).name).toBe('AbortError')
        expect(error).not.toBeInstanceOf(ApiError)
        expect(fetcher.mock.calls[0]?.[1]?.signal?.aborted).toBe(true)
        expect(dispatched).not.toHaveBeenCalled()
    })

    it('honours an explicit timeoutMs override for uploads', async () => {
        hangingFetch()
        const started = Date.now()
        const error = await request('/api/upload', {
            method: 'POST',
            json: { a: 1 },
            timeoutMs: 30,
        }).catch((v: unknown) => v)
        expect(error).toBeInstanceOf(ApiError)
        expect(Date.now() - started).toBeLessThan(2_000)
    })
})
