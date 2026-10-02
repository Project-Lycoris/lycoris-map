import { expect, it } from 'vitest'
import { z } from 'zod'
import { ApiError } from './ApiError'
import { readError } from './readError'
it.each([
    [401, 'session expired'],
    [403, 'permission'],
    [404, 'unavailable'],
    [408, 'timed out'],
    [429, 'Too many requests'],
    [500, 'service'],
    [504, 'timed out'],
])('classifies HTTP %s without leaking the response body', (status, text) => {
    expect(
        readError(new ApiError(Number(status), '<html>internal sensitive response</html>')),
    ).toContain(String(text))
})
it('distinguishes network, timeout, malformed DTO and cancellation', () => {
    expect(readError(ApiError.network('fetch failed'))).toContain('Network unavailable')
    expect(readError(ApiError.network('deadline', undefined, 'timeout'))).toContain('timed out')
    const invalid = z.object({ id: z.number() }).safeParse({ id: 'bad' })
    expect(readError(invalid.error)).toContain('could not be read')
    expect(readError(new DOMException('Superseded', 'AbortError'))).toBeNull()
})
