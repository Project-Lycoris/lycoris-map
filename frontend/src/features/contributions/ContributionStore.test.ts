import { QueryClient } from '@tanstack/react-query'
import { afterEach, expect, it, vi } from 'vitest'
import { ContributionStore } from './ContributionStore'
import { SessionStore } from '@/features/auth/SessionStore'
import * as sessionApi from '@/shared/api/session'
import { ApiError } from '@/shared/api/ApiError'
import { syntheticPlace } from '@/features/dev/placeFixtures'
import { draftText, emptyContributionDraft } from './draft'
import { checkPhotoFile, checkPhotoSize } from './photo'

afterEach(() => vi.restoreAllMocks())
async function setup() {
    vi.spyOn(sessionApi, 'fetchMe').mockResolvedValue({
        publicId: 'A',
        username: 'Synthetic',
        nickname: null,
        email: null,
        avatarUrl: null,
        pronouns: null,
        signature: null,
    })
    const client = new QueryClient(),
        session = new SessionStore(client)
    await session.refresh()
    const scope = session.getSnapshot().scope!
    const marker = syntheticPlace({ id: 51, reviewStatus: 'PENDING', isPublic: true })
    const api = {
        createMarker: vi.fn().mockResolvedValue(marker),
        proposeMarkerEdit: vi.fn().mockResolvedValue(marker),
    }
    const upload = vi.fn().mockResolvedValue(undefined)
    const store = new ContributionStore(client, session, api, async () => undefined, upload)
    store.change({ ...emptyContributionDraft, title: 'Synthetic place', category: 'toilet' })
    store.setPoint({ lat: 30, lng: 120 })
    return { store, api, upload, session, scope, client, marker }
}
it('freezes create UUID, content and point after a lost response, including reopening the draft', async () => {
    const { store, api, scope } = await setup()
    api.createMarker.mockRejectedValueOnce(new ApiError(408, 'response timed out'))
    await store.submit(scope)
    expect(store.getSnapshot().phase).toBe('save-uncertain')
    store.beginCreate('zh')
    store.change({ ...emptyContributionDraft, title: 'different', category: 'medical' })
    store.setPoint({ lat: 1, lng: 2 })
    await store.submit(scope)
    expect(api.createMarker.mock.calls[1]![0]).toEqual(api.createMarker.mock.calls[0]![0])
    expect(store.getSnapshot().phase).toBe('complete')
})
it('deduplicates double submit and invalid drafts never write', async () => {
    const { store, api, scope } = await setup()
    const first = store.submit(scope)
    await store.submit(scope)
    await first
    expect(api.createMarker).toHaveBeenCalledTimes(1)
    store.beginCreate('en')
    await store.submit(scope)
    expect(api.createMarker).toHaveBeenCalledTimes(1)
    expect(store.getSnapshot().error).toMatch(/title/)
})
it('photo-only recovery keeps the marker receipt and photo UUID, without repeating marker creation', async () => {
    const { store, api, upload, scope, client } = await setup()
    await store.photo(new File(['photo'], 'synthetic.png', { type: 'image/png' }))
    upload.mockRejectedValueOnce(ApiError.network('lost'))
    await store.submit(scope)
    expect(store.getSnapshot().phase).toBe('photo-paused')
    await store.submit(scope)
    expect(api.createMarker).toHaveBeenCalledTimes(1)
    expect(upload.mock.calls[0]!.slice(0, 3)).toEqual(upload.mock.calls[1]!.slice(0, 3))
    expect(store.getSnapshot().phase).toBe('complete')
    expect(client.getQueryCache().getAll()).toHaveLength(0)
})
it('edit proposals preserve source language, privacy and fixed coordinates without writing computed active', async () => {
    const { store, api, scope } = await setup()
    const base = syntheticPlace({
        contentLanguage: 'zh',
        isPublic: false,
        isActive: false,
        description: 'Old',
        openTimeStart: '09:00',
        openTimeEnd: '17:00',
    })
    store.beginEdit(base)
    store.setPoint({ lat: 0, lng: 0 })
    store.change({
        ...store.getSnapshot().draft,
        title: '新标题',
        description: '',
        openingHour: '',
        openingMinute: '',
        closingHour: '',
        closingMinute: '',
    })
    await store.submit(scope)
    expect(store.getSnapshot().point).toEqual({ lat: base.lat, lng: base.lng })
    expect(api.proposeMarkerEdit.mock.calls[0]![1]).toEqual({
        title: '新标题',
        category: base.category,
        categories: [base.category],
        openingHoursNote: '',
        venueType: null,
        description: '',
        language: 'zh',
        isPublic: false,
        openTimeStart: '',
        openTimeEnd: '',
    })
    expect(api.createMarker).not.toHaveBeenCalled()
})
it('does not create redundant text proposals for a photo-only edit', async () => {
    const { store, api, scope, upload, marker } = await setup()
    store.beginEdit(marker)
    await store.photo(new File(['photo'], 'photo.png', { type: 'image/png' }))
    await store.submit(scope)
    expect(api.proposeMarkerEdit).not.toHaveBeenCalled()
    expect(upload).toHaveBeenCalledTimes(1)
})
it('requires explicit resend for an uncertain non-idempotent text edit', async () => {
    const { store, api, scope, marker } = await setup()
    store.beginEdit(marker)
    store.change({ ...store.getSnapshot().draft, title: 'Edited' })
    api.proposeMarkerEdit.mockRejectedValueOnce(ApiError.network('lost'))
    await store.submit(scope)
    await store.submit(scope)
    expect(api.proposeMarkerEdit).toHaveBeenCalledTimes(1)
    await store.submit(scope, true)
    expect(api.proposeMarkerEdit).toHaveBeenCalledTimes(2)
})
it('account change cancels background photo work and discards private drafts and late results', async () => {
    const { store, upload, session, scope } = await setup()
    let entered!: () => void, finish!: () => void
    const ready = new Promise<void>((resolve) => {
        entered = resolve
    })
    const pending = new Promise<void>((resolve) => {
        finish = resolve
    })
    upload.mockImplementation(async () => {
        entered()
        await pending
    })
    await store.photo(new File(['photo'], 'photo.png', { type: 'image/png' }))
    const submission = store.submit(scope)
    await ready
    const signal = upload.mock.calls[0]![4] as AbortSignal
    session.externalChange()
    store.clearStale()
    expect(signal.aborted).toBe(true)
    finish()
    await submission
    expect(store.getSnapshot()).toMatchObject({
        phase: 'draft',
        saved: null,
        draft: { title: '', photo: null },
    })
})
it('validates paired times, Unicode title limits and decoded photo dimensions', () => {
    const draft = {
        ...emptyContributionDraft,
        title: '😊'.repeat(120),
        category: 'toilet' as const,
    }
    expect(draftText(draft, 'en').title).toBe(draft.title)
    expect(() => draftText({ ...draft, openingHour: '9' }, 'en')).toThrow(/both/)
    expect(() => draftText({ ...draft, title: draft.title + 'x' }, 'en')).toThrow(/120/)
    expect(() => checkPhotoFile(new File(['a'], 'fake.svg', { type: 'image/svg+xml' }))).toThrow(
        /JPEG/,
    )
    expect(() => checkPhotoSize(10000, 3000)).toThrow(/25 megapixels/)
})
it('keeps an uncertain create frozen even when a later recovery is rejected', async () => {
    const { store, api, scope } = await setup()
    api.createMarker
        .mockRejectedValueOnce(ApiError.network('lost'))
        .mockRejectedValueOnce(new ApiError(403, 'temporarily forbidden'))
    await store.submit(scope)
    await store.submit(scope)
    expect(store.getSnapshot().phase).toBe('save-uncertain')
    store.change({ ...emptyContributionDraft, title: 'New place', category: 'medical' })
    store.setPoint({ lat: 0, lng: 0 })
    await store.submit(scope)
    expect(api.createMarker.mock.calls[2]![0]).toEqual(api.createMarker.mock.calls[0]![0])
})
it('can reopen busy work and dispose cancels it without accepting a late result', async () => {
    const { store, upload, scope } = await setup()
    let entered!: () => void, finish!: () => void
    const ready = new Promise<void>((resolve) => {
        entered = resolve
    })
    const pending = new Promise<void>((resolve) => {
        finish = resolve
    })
    upload.mockImplementation(async () => {
        entered()
        await pending
    })
    await store.photo(new File(['photo'], 'photo.png', { type: 'image/png' }))
    const submission = store.submit(scope)
    await ready
    const before = store.getSnapshot()
    expect(store.beginCreate('zh')).toBe(true)
    expect(store.getSnapshot()).toBe(before)
    store.dispose()
    expect((upload.mock.calls[0]![4] as AbortSignal).aborted).toBe(true)
    finish()
    await submission
    expect(store.getSnapshot()).toMatchObject({ phase: 'photo-saving', uploadedCount: 0 })
})
it('allows discarding a rejected image while preserving the saved place', async () => {
    const { store, scope, upload, api } = await setup()
    await store.photo(new File(['photo'], 'photo.png', { type: 'image/png' }))
    upload.mockRejectedValueOnce(new ApiError(400, 'Invalid image'))
    await store.submit(scope)
    expect(store.getSnapshot().phase).toBe('photo-error')
    await store.photo(null)
    expect(store.getSnapshot().phase).toBe('complete')
    expect(api.createMarker).toHaveBeenCalledTimes(1)
})

it('submits Chinese content from an English UI and an old English draft', async () => {
    const { store, api, scope } = await setup()
    store.beginCreate('en')
    store.change({ ...emptyContributionDraft, title: '中文点位', category: 'toilet' })
    store.setPoint({ lat: 30, lng: 120 })
    await store.submit(scope)
    expect(api.createMarker.mock.calls[0]![0].language).toBe('zh')
    expect(
        draftText({ ...emptyContributionDraft, title: '中文修改', category: 'toilet' }, 'en')
            .language,
    ).toBe('zh')
})

it('restores multiple photos and the saved marker after a reload without repeating completed uploads', async () => {
    const { client, session, api, scope, marker } = await setup()
    const rows = new Map<string, import('./DraftJournal').SavedDraft>()
    const journal: import('./DraftJournal').DraftJournal = {
        list: async (owner) => [...rows.values()].filter((row) => row.owner === owner),
        put: async (row) => {
            rows.set(row.id, row)
        },
        remove: async (id, owner) => {
            if (rows.get(id)?.owner === owner) rows.delete(id)
        },
    }
    const uploader = vi
        .fn()
        .mockResolvedValueOnce(undefined)
        .mockRejectedValueOnce(ApiError.network('lost'))
    const first = new ContributionStore(client, session, api, async () => {}, uploader, journal)
    first.change({ ...emptyContributionDraft, title: 'Two photos', category: 'toilet' })
    first.setPoint({ lat: 30, lng: 120 })
    const photos = [
        new File(['a'], 'a.png', { type: 'image/png' }),
        new File(['b'], 'b.png', { type: 'image/png' }),
    ]
    await first.addPhotos(photos)
    await first.submit(scope)
    expect(first.getSnapshot()).toMatchObject({
        phase: 'photo-paused',
        uploadedCount: 1,
        saved: marker,
    })
    const id = first.getSnapshot().requestId
    await first.newDraft() // Flush pending persistence; the paused receipt remains in the library.
    const upload = vi.fn().mockResolvedValue(undefined)
    const restored = new ContributionStore(client, session, api, async () => {}, upload, journal)
    expect(await restored.resumeDraft(id)).toBe(true)
    await restored.submit(scope)
    expect(api.createMarker).toHaveBeenCalledTimes(1)
    expect(upload).toHaveBeenCalledTimes(1)
    expect(upload.mock.calls[0]!.slice(0, 3)).toEqual(uploader.mock.calls[1]!.slice(0, 3))
    expect(restored.getSnapshot().phase).toBe('complete')
    await restored.newDraft()
    expect(rows.size).toBe(0)
})

it('keeps an interrupted draft private across account changes and restores it for its owner', async () => {
    const { client, session, api, scope } = await setup()
    const rows = new Map<string, import('./DraftJournal').SavedDraft>()
    const journal: import('./DraftJournal').DraftJournal = {
        list: async (owner) => [...rows.values()].filter((row) => row.owner === owner),
        put: async (row) => {
            rows.set(row.id, row)
        },
        remove: async (id, owner) => {
            if (rows.get(id)?.owner === owner) rows.delete(id)
        },
    }
    const store = new ContributionStore(client, session, api, async () => {}, vi.fn(), journal)
    store.change({ ...emptyContributionDraft, title: 'Private draft', category: 'toilet' })
    store.setPoint({ lat: 31, lng: 121 })
    const id = store.getSnapshot().requestId
    await store.newDraft()
    vi.mocked(sessionApi.fetchMe).mockResolvedValue({
        ...session.getSnapshot().user!,
        publicId: 'B',
    })
    await session.refresh()
    store.clearStale()
    await store.refreshDrafts()
    expect(store.getSnapshot().drafts).toEqual([])
    expect(await store.resumeDraft(id)).toBe(false)
    expect(rows.get(id)?.owner).toBe(scope.publicId)
    vi.mocked(sessionApi.fetchMe).mockResolvedValue({
        ...session.getSnapshot().user!,
        publicId: 'A',
    })
    await session.refresh()
    store.clearStale()
    expect(await store.resumeDraft(id)).toBe(true)
    expect(store.getSnapshot().draft.title).toBe('Private draft')
})

it('does not submit when the durable checkpoint fails, and reports storage failure', async () => {
    const { client, session, api, scope } = await setup()
    const journal: import('./DraftJournal').DraftJournal = {
        list: async () => [],
        put: async () => {
            throw new Error('quota exceeded')
        },
        remove: async () => {},
    }
    const store = new ContributionStore(client, session, api, async () => {}, vi.fn(), journal)
    store.change({ ...emptyContributionDraft, title: 'Keep my work', category: 'toilet' })
    store.setPoint({ lat: 30, lng: 120 })
    await store.submit(scope)
    expect(api.createMarker).not.toHaveBeenCalled()
    expect(store.getSnapshot().persistenceError).toContain('Could not save')
    expect(store.getSnapshot().draft.title).toBe('Keep my work')
})
