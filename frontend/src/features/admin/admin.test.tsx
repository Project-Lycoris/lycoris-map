import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter } from 'react-router'
import { SessionProvider, useSession } from '@/features/auth/SessionProvider'
import { AccountFlowProvider } from '@/features/auth/AccountFlow'
import { LanguageProvider } from '@/shared/i18n'
import * as sessionApi from '@/shared/api/session'
import { ApiError } from '@/shared/api/ApiError'
import type { User } from '@/shared/api/users'
import { syntheticPlace } from '@/features/dev/placeFixtures'
import * as api from './api'
import AdminPage from './AdminPage'
import { deniedAccess } from './useAdminAccess'
const user: User = {
    publicId: 'A',
    username: 'synthetic-admin',
    email: null,
    nickname: null,
    avatarUrl: null,
    pronouns: null,
    signature: null,
}
let owner: User | null
let client: QueryClient
beforeEach(() => {
    owner = user
    vi.spyOn(sessionApi, 'fetchMe').mockImplementation(async () => owner)
    vi.spyOn(api, 'readUsers').mockResolvedValue({
        page: 0,
        size: 10,
        totalPages: 0,
        totalElements: 0,
        items: [],
    })
    vi.spyOn(api, 'readMarkers').mockResolvedValue([
        syntheticPlace({ title: 'Review synthetic', reviewStatus: 'PENDING' }),
    ])
    vi.spyOn(api, 'readEdits').mockResolvedValue([])
    vi.spyOn(api, 'readImages').mockResolvedValue([])
    vi.spyOn(api, 'moderate').mockResolvedValue(undefined)
})
afterEach(() => {
    cleanup()
    client?.clear()
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
})
function Observer() {
    const session = useSession()
    return (
        <>
            <output data-testid="identity">
                {session.status}:{session.user?.publicId}
            </output>
            <button onClick={() => session.store?.externalChange()}>Recheck identity</button>
        </>
    )
}
function mount(path = '/admin') {
    client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
    return render(
        <MemoryRouter initialEntries={[path]}>
            <QueryClientProvider client={client}>
                <LanguageProvider initialPreference="en">
                    <SessionProvider>
                        <AccountFlowProvider>
                            <AdminPage />
                            <Observer />
                        </AccountFlowProvider>
                    </SessionProvider>
                </LanguageProvider>
            </QueryClientProvider>
        </MemoryRouter>,
    )
}
it('keeps anonymous visitors outside protected endpoints', async () => {
    owner = null
    mount()
    await screen.findByRole('heading', { name: 'Login' })
    expect(api.readUsers).not.toHaveBeenCalled()
    expect(api.readMarkers).not.toHaveBeenCalled()
})
it('distinguishes ordinary users and unknown 403s without logging either out', async () => {
    vi.mocked(api.readUsers).mockRejectedValue(
        new ApiError(403, 'HTTP 403', { accessDenied: true }),
    )
    mount()
    await screen.findByText('Access denied.')
    expect(screen.getByTestId('identity')).toHaveTextContent('authenticated:A')
    expect(api.readMarkers).not.toHaveBeenCalled()
    vi.mocked(api.readUsers).mockRejectedValue(new ApiError(403, 'Unrecognized response'))
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    await screen.findByText('Could not confirm access. Try again.')
    expect(screen.getByTestId('identity')).toHaveTextContent('authenticated:A')
})
it('opens the review queue for an authorized login without a second password', async () => {
    mount()
    await screen.findByRole('heading', { name: 'Review synthetic' })
    expect(screen.queryByLabelText('Admin passcode')).not.toBeInTheDocument()
})
it('clears queues when the admin role is revoked, retaining the ordinary session', async () => {
    mount()
    await screen.findByRole('heading', { name: 'Review synthetic' })
    vi.mocked(api.readMarkers).mockRejectedValue(
        new ApiError(403, 'Forbidden', { accessDenied: true }),
    )
    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }))
    await screen.findByText('Access denied.')
    expect(screen.queryByText('Review synthetic')).not.toBeInTheDocument()
    expect(screen.getByTestId('identity')).toHaveTextContent('authenticated:A')
    expect(client.getQueriesData({ predicate: (q) => q.queryKey.includes('review') })).toEqual([])
})
it('refreshes after approval and never automatically replays an uncertain write', async () => {
    mount()
    await screen.findByRole('heading', { name: 'Review synthetic' })
    vi.mocked(api.moderate).mockRejectedValue(ApiError.network('lost response'))
    vi.mocked(api.readMarkers).mockResolvedValue([])
    fireEvent.click(screen.getByRole('button', { name: 'Approve' }))
    expect(api.moderate).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Confirm' }))
    await screen.findByText(
        'The outcome could not be confirmed. Refresh and inspect the item before trying again.',
    )
    await screen.findByText('No items.')
    expect(api.moderate).toHaveBeenCalledTimes(1)
})
it('does not render a delayed private queue after an account switch', async () => {
    let finish!: (value: ReturnType<typeof syntheticPlace>[]) => void
    vi.mocked(api.readMarkers).mockImplementationOnce(
        () =>
            new Promise((resolve) => {
                finish = resolve
            }),
    )
    mount()
    await waitFor(() => expect(api.readMarkers).toHaveBeenCalled())
    owner = { ...user, publicId: 'B' }
    vi.mocked(api.readUsers).mockRejectedValue(
        new ApiError(403, 'HTTP 403', { accessDenied: true }),
    )
    fireEvent.click(screen.getByText('Recheck identity'))
    await act(async () => finish([syntheticPlace({ title: 'Old secret queue' })]))
    await screen.findByText('Access denied.')
    expect(screen.queryByText('Old secret queue')).not.toBeInTheDocument()
    expect(client.getQueriesData({ queryKey: ['private', 'A'] })).toEqual([])
})
it('renders at most one page of a large moderation queue', async () => {
    vi.mocked(api.readMarkers).mockResolvedValue(
        Array.from({ length: 400 }, (_, i) =>
            syntheticPlace({ id: i + 1, title: `Review item ${i}` }),
        ),
    )
    const { container } = mount()
    await screen.findByText('Review item 0')
    expect(container.querySelectorAll('.admin-card')).toHaveLength(20)
    fireEvent.click(screen.getByRole('button', { name: 'Next' }))
    await screen.findByText('Review item 20')
    expect(container.querySelectorAll('.admin-card')).toHaveLength(20)
})
it('does not classify service failure as an ordinary account', () => {
    expect(deniedAccess(new ApiError(503, 'unavailable'))).toBeNull()
    expect(deniedAccess(new ApiError(403, 'unrecognized'))).toBe('unavailable')
})

it('preserves the actual content language when editing fallback text', async () => {
    vi.mocked(api.readMarkers).mockResolvedValue([
        syntheticPlace({ title: '原文', contentLanguage: 'zh' }),
    ])
    const save = vi.spyOn(api, 'editMarker').mockResolvedValue(syntheticPlace())
    mount('/admin/all')
    await screen.findByRole('heading', { name: '原文' })
    fireEvent.click(screen.getByRole('button', { name: 'Edit' }))
    fireEvent.change(screen.getByLabelText('Opening Time'), { target: { value: '10:00' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    fireEvent.click(screen.getByRole('button', { name: 'Confirm' }))
    await waitFor(() =>
        expect(save).toHaveBeenCalledWith(
            1,
            expect.objectContaining({ title: '原文', language: 'zh', openTimeStart: '10:00' }),
            expect.any(AbortSignal),
        ),
    )
})
it('cancels confirmation with Escape and restores its trigger focus', async () => {
    mount()
    await screen.findByRole('heading', { name: 'Review synthetic' })
    const approve = screen.getByRole('button', { name: 'Approve' })
    act(() => approve.focus())
    fireEvent.click(approve)
    expect(screen.getByRole('button', { name: 'Cancel' })).toHaveFocus()
    fireEvent.keyDown(window, { key: 'Escape' })
    await waitFor(() => expect(approve).toHaveFocus())
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument()
    expect(api.moderate).not.toHaveBeenCalled()
})

it('disables and restores a place with confirmation while preventing edits to disabled places', async () => {
    let marker = syntheticPlace({ title: 'Recoverable place', deactivated: false })
    vi.mocked(api.readMarkers).mockImplementation(async () => [marker])
    const disable = vi.spyOn(api, 'deactivateMarker').mockImplementation(async () => {
        marker = { ...marker, deactivated: true }
    })
    const restore = vi.spyOn(api, 'restoreMarker').mockImplementation(async () => {
        marker = { ...marker, deactivated: false }
    })
    mount('/admin/all')
    fireEvent.click(await screen.findByRole('button', { name: 'Disable' }))
    expect(disable).not.toHaveBeenCalled()
    expect(screen.getByRole('alertdialog')).toHaveTextContent('All data is kept')
    fireEvent.click(screen.getByRole('button', { name: 'Confirm' }))
    await screen.findByRole('button', { name: 'Restore' })
    expect(disable).toHaveBeenCalledWith(marker.id, expect.any(AbortSignal))
    expect(screen.getByRole('button', { name: 'Edit' })).toBeDisabled()
    expect(screen.getByRole('button', { name: 'Approve' })).toBeDisabled()
    fireEvent.click(screen.getByRole('button', { name: 'Restore' }))
    expect(screen.getByRole('alertdialog')).toHaveTextContent(
        'previous visibility and review status',
    )
    fireEvent.click(screen.getByRole('button', { name: 'Confirm' }))
    await screen.findByRole('button', { name: 'Disable' })
    expect(restore).toHaveBeenCalledWith(marker.id, expect.any(AbortSignal))
    expect(screen.getByRole('button', { name: 'Edit' })).toBeEnabled()
})

it('does not send a supposedly recoverable deletion to an older backend', async () => {
    const disable = vi.spyOn(api, 'deactivateMarker')
    mount('/admin/all')
    const waiting = await screen.findByRole('button', { name: 'Server update required' })
    expect(waiting).toBeDisabled()
    fireEvent.click(waiting)
    expect(disable).not.toHaveBeenCalled()
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument()
})

it('keeps the dashboard usable after approval without rerunning the access gate', async () => {
    mount()
    await screen.findByRole('heading', { name: 'Review synthetic' })
    // Moderation reauthorizes on the server; refreshing its data must not reset
    // the whole dashboard because an unrelated users-list probe is unavailable.
    vi.mocked(api.readUsers).mockRejectedValue(new ApiError(503, 'unavailable'))
    vi.mocked(api.readMarkers).mockResolvedValue([])
    fireEvent.click(screen.getByRole('button', { name: 'Approve' }))
    fireEvent.click(screen.getByRole('button', { name: 'Confirm' }))
    await screen.findByText('No items.')
    expect(screen.getByRole('button', { name: 'Refresh' })).toBeEnabled()
    expect(screen.getByRole('link', { name: 'Users' })).toBeInTheDocument()
    expect(api.readUsers).toHaveBeenCalledTimes(1)
    expect(api.moderate).toHaveBeenCalledTimes(1)
})

it('groups content and images for each place and supports consecutive approvals', async () => {
    const image: api.ImageProposal = {
        id: 1,
        markerId: 1,
        markerTitle: 'Review synthetic',
        proposerUsername: 'synthetic',
        proposerPublicId: null,
        imageUrl: '/uploads/markers/synthetic.jpg',
        status: 'PENDING',
        createdAt: '2026-09-15T01:00:00Z',
    }
    vi.mocked(api.readImages).mockResolvedValue([image])
    vi.mocked(api.moderate).mockImplementation(async (kind) => {
        if (kind === 'markers') vi.mocked(api.readMarkers).mockResolvedValue([])
        else vi.mocked(api.readImages).mockResolvedValue([])
    })
    const { container } = mount()
    await screen.findByText('Photo submission')
    expect(container.querySelectorAll('.admin-place-group')).toHaveLength(1)
    expect(container.querySelectorAll('.admin-card')).toHaveLength(2)
    const first = screen.getAllByRole('article')[0]!
    fireEvent.click(within(first).getByRole('button', { name: 'Approve' }))
    fireEvent.click(screen.getByRole('button', { name: 'Confirm' }))
    await waitFor(() => expect(screen.queryByText('New place')).not.toBeInTheDocument())
    expect(screen.getByText('Photo submission')).toBeInTheDocument()
    await waitFor(() => expect(screen.getByRole('button', { name: 'Approve' })).toBeEnabled())
    fireEvent.click(screen.getByRole('button', { name: 'Approve' }))
    expect(screen.getByRole('alertdialog')).toHaveTextContent('Photo submission')
    fireEvent.click(screen.getByRole('button', { name: 'Confirm' }))
    await screen.findByText('No items.')
    expect(vi.mocked(api.moderate).mock.calls.map(([kind, id]) => [kind, id])).toEqual([
        ['markers', 1],
        ['images', 1],
    ])
})

it('keeps navigation and refresh available when the post-approval queue read fails', async () => {
    mount()
    await screen.findByRole('heading', { name: 'Review synthetic' })
    vi.mocked(api.readMarkers).mockRejectedValueOnce(new ApiError(503, 'unavailable'))
    fireEvent.click(screen.getByRole('button', { name: 'Approve' }))
    fireEvent.click(screen.getByRole('button', { name: 'Confirm' }))
    await screen.findByText('The request failed. Refresh and try again.')
    await waitFor(() => expect(screen.getByRole('button', { name: 'Refresh' })).toBeEnabled())
    expect(screen.getByRole('link', { name: 'All places' })).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }))
    await screen.findByRole('heading', { name: 'Review synthetic' })
    expect(api.moderate).toHaveBeenCalledTimes(1)
})

it('has no bulk image-cleanup action in all places', async () => {
    mount('/admin/all')
    await screen.findByRole('heading', { name: 'Review synthetic' })
    expect(screen.queryByRole('button', { name: /clean|clear missing/i })).not.toBeInTheDocument()
})

it('releases the mutation lock even if cache refresh itself rejects', async () => {
    mount()
    await screen.findByRole('heading', { name: 'Review synthetic' })
    vi.spyOn(client, 'invalidateQueries').mockRejectedValueOnce(new Error('refresh interrupted'))
    fireEvent.click(screen.getByRole('button', { name: 'Approve' }))
    fireEvent.click(screen.getByRole('button', { name: 'Confirm' }))
    await screen.findByText(
        'Saved, but the list could not refresh. Refresh before reviewing another item.',
    )
    expect(screen.getByRole('button', { name: 'Refresh' })).toBeEnabled()
    expect(api.moderate).toHaveBeenCalledTimes(1)
    vi.mocked(api.readMarkers).mockResolvedValue([])
    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }))
    await screen.findByText('No items.')
})
