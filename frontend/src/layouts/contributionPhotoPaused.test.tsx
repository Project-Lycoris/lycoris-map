import { afterEach, expect, it, vi } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { QueryClient } from '@tanstack/react-query'
import { ContributionForm } from './ContributionForm'
import { ContributionStore } from '@/features/contributions/ContributionStore'
import { SessionStore } from '@/features/auth/SessionStore'
import { emptyContributionDraft } from '@/features/contributions/draft'
import * as sessionApi from '@/shared/api/session'
import { ApiError } from '@/shared/api/ApiError'
import { syntheticPlace } from '@/features/dev/placeFixtures'
import { UiLanguage } from '@/shared/i18n/ui'

afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
})

// B2-1: a real store reaches `photo-paused` with the prefixed message that
// used to recurse inside `localize()`. Rendering the form must not crash.
async function photoPausedStore() {
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
    await store.photo(new File(['photo'], 'synthetic.png', { type: 'image/png' }))
    upload.mockRejectedValueOnce(new ApiError(500, 'This place is unavailable.'))
    await store.submit(scope)
    expect(store.getSnapshot().phase).toBe('photo-paused')
    return store
}

function form(store: ContributionStore) {
    const snapshot = store.getSnapshot()
    return (
        <ContributionForm
            draft={snapshot.draft}
            onChange={() => {}}
            close={() => {}}
            point={snapshot.point}
            state={snapshot}
        />
    )
}

it('renders the form while a photo upload is paused instead of overflowing', async () => {
    const store = await photoPausedStore()
    render(form(store))
    expect(screen.getByRole('alert')).toHaveTextContent(
        'The place step is saved. Photo upload paused: This place is unavailable.',
    )
})

it('translates the paused-upload notice for Chinese', async () => {
    const store = await photoPausedStore()
    render(<UiLanguage language="zh">{form(store)}</UiLanguage>)
    expect(screen.getByRole('alert')).toHaveTextContent(
        '点位已保存，照片上传暂停：此点位暂不可用。',
    )
})
