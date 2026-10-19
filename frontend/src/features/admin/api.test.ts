import { request } from '@/shared/api/transport'
import { ApiError } from '@/shared/api/ApiError'
import * as api from './api'
import { deniedAccess } from './useAdminAccess'
afterEach(() => vi.unstubAllGlobals())
const json = (data: unknown, status = 200) =>
    new Response(JSON.stringify(data), { status, headers: { 'Content-Type': 'application/json' } })
it('accepts nullable historical usernames/emails, retaining the real pagination', async () => {
    const fetcher = vi.fn().mockResolvedValue(
        json({
            page: 2,
            size: 10,
            totalPages: 3,
            totalElements: 21,
            items: [
                {
                    id: 1,
                    publicId: 'historical',
                    username: null,
                    email: null,
                    nickname: null,
                    avatarUrl: null,
                    pronouns: null,
                    signature: null,
                    role: 'USER',
                    deleted: true,
                    deletedAt: '2026-01-01T00:00:00Z',
                },
            ],
        }),
    )
    vi.stubGlobal('fetch', fetcher)
    const page = await api.readUsers(2, 'history', new AbortController().signal)
    expect(page.items[0]?.username).toBeNull()
    expect(fetcher).toHaveBeenCalledWith(
        '/api/admin/users?page=2&size=10&q=history',
        expect.objectContaining({ cache: 'no-store', credentials: 'include' }),
    )
})
it('classifies only the exact backend role denial shape, preserving other 403 meanings', async () => {
    for (const [response, kind] of [
        [
            json(
                {
                    timestamp: '2026-01-01T00:00:00Z',
                    status: 403,
                    error: 'Forbidden',
                    path: '/api/admin/users',
                },
                403,
            ),
            'denied',
        ],
        [new Response('需要二级密码', { status: 403 }), 'unavailable'],
        [new Response('二级密码已过期，请重新验证', { status: 403 }), 'unavailable'],
        [json({ status: 403, error: 'Forbidden' }, 403), 'unavailable'],
    ] as const) {
        vi.stubGlobal('fetch', vi.fn().mockResolvedValue(response))
        const error = await request('/api/admin/users').catch((error) => error)
        expect(error).toBeInstanceOf(ApiError)
        expect(deniedAccess(error)).toBe(kind)
    }
})
it('uses the exact moderation and user action endpoints without inventing password payloads', async () => {
    const fetcher = vi.fn().mockImplementation(async () => json({ message: 'ok' }))
    vi.stubGlobal('fetch', fetcher)
    const signal = new AbortController().signal
    await api.moderate('edits', 7, 'reject', 'en', signal)
    await api.moderate('images', 8, 'approve', 'zh', signal)
    await api.changeUser(9, 'reset-password', signal)
    await api.changeUser(9, 'disable', signal)
    await api.changeUser(9, 'restore', signal)
    await api.deactivateMarker(10, signal)
    await api.restoreMarker(10, signal)
    expect(fetcher.mock.calls.map((call) => [call[0], call[1].method, call[1].body])).toEqual([
        ['/api/admin/markers/edit-proposals/7/reject?lang=en', 'POST', undefined],
        ['/api/admin/markers/image-proposals/8/approve?lang=zh', 'POST', undefined],
        ['/api/admin/users/9/reset-password', 'POST', undefined],
        ['/api/admin/users/9', 'DELETE', undefined],
        ['/api/admin/users/9/restore', 'POST', undefined],
        ['/api/admin/markers/10', 'DELETE', undefined],
        ['/api/admin/markers/10/restore', 'POST', undefined],
    ])
    await expect(api.changeUser(Number.MAX_SAFE_INTEGER + 1, 'disable', signal)).rejects.toThrow()
    expect(fetcher).toHaveBeenCalledTimes(7)
})

it('reads pending submissions with submitter statistics', async () => {
    const fetcher = vi.fn().mockResolvedValue(
        json([
            {
                id: 1,
                version: 1,
                lat: 31.2,
                lng: 121.4,
                category: 'accessible_toilet',
                categories: ['accessible_toilet'],
                title: 'Pending',
                description: null,
                sourceLanguage: 'zh',
                contentLanguage: 'zh',
                isPublic: true,
                username: 'alice',
                userPublicId: null,
                clientRequestId: null,
                isActive: true,
                openTimeStart: '08:00',
                openTimeEnd: '22:00',
                reviewStatus: 'PENDING',
                lastEditedBy: null,
                lastEditedByPublicId: null,
                lastEditedByOwner: false,
                markImage: null,
                createdAt: '2026-10-18T06:50:15Z',
                updatedAt: '2026-10-18T06:50:15Z',
                proposerSubmissionStats: { approved: 3, rejected: 1, pending: 2 },
            },
        ]),
    )
    vi.stubGlobal('fetch', fetcher)
    const markers = await api.readPendingMarkers(new AbortController().signal)
    expect(markers[0]!.proposerSubmissionStats).toEqual({
        approved: 3,
        rejected: 1,
        pending: 2,
    })
    expect(fetcher).toHaveBeenCalledWith(
        '/api/admin/markers/pending?lang=zh',
        expect.objectContaining({ credentials: 'include', cache: 'no-store' }),
    )
})

it('sends reason and changes bodies only when supplied', async () => {
    const fetcher = vi.fn().mockImplementation(async () => json({ results: [] }))
    vi.stubGlobal('fetch', fetcher)
    const signal = new AbortController().signal
    await api.moderate('markers', 1, 'approve', 'zh', signal, {
        changes: {
            title: 'Corrected',
            category: 'accessible_toilet',
            categories: ['accessible_toilet'],
            description: '',
            language: 'zh',
            isPublic: true,
            openTimeStart: '08:00',
            openTimeEnd: '22:00',
        },
    })
    await api.moderate('edits', 2, 'reject', 'zh', signal, { reason: 'duplicate' })
    await api.moderate('images', 3, 'approve', 'zh', signal)
    await api.batchModerate(
        [
            { kind: 'marker', id: 1, decision: 'approve' },
            { kind: 'edit', id: 2, decision: 'reject', reason: 'duplicate' },
        ],
        signal,
    )
    expect(fetcher.mock.calls.map(([url, init]) => [url, (init as RequestInit).body])).toEqual([
        ['/api/admin/markers/1/approve?lang=zh', expect.stringContaining('"changes"')],
        ['/api/admin/markers/edit-proposals/2/reject?lang=zh', '{"reason":"duplicate"}'],
        ['/api/admin/markers/image-proposals/3/approve?lang=zh', undefined],
        [
            '/api/admin/moderation/batch',
            '{"items":[{"kind":"marker","id":1,"decision":"approve"},{"kind":"edit","id":2,"decision":"reject","reason":"duplicate"}]}',
        ],
    ])
})
