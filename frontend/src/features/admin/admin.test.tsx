import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
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
import * as privatePlaces from '@/shared/api/privatePlaces'
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
    vi.spyOn(api, 'readPendingMarkers').mockResolvedValue([
        {
            ...syntheticPlace({ title: 'Review synthetic', reviewStatus: 'PENDING' }),
            proposerSubmissionStats: { approved: 1, rejected: 0, pending: 1 },
        },
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
    vi.mocked(api.readPendingMarkers).mockRejectedValue(
        new ApiError(403, 'Forbidden', { accessDenied: true }),
    )
    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }))
    await screen.findByText('Access denied.')
    expect(screen.queryByText('Review synthetic')).not.toBeInTheDocument()
    expect(screen.getByTestId('identity')).toHaveTextContent('authenticated:A')
    expect(client.getQueriesData({ predicate: (q) => q.queryKey.includes('review') })).toEqual([])
})
it('does not render a delayed private queue after an account switch', async () => {
    let finish!: (value: ReturnType<typeof syntheticPlace>[]) => void
    vi.mocked(api.readPendingMarkers).mockImplementationOnce(
        () =>
            new Promise((resolve) => {
                finish = resolve
            }),
    )
    mount()
    await waitFor(() => expect(api.readPendingMarkers).toHaveBeenCalled())
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
it('does not classify service failure as an ordinary account', () => {
    expect(deniedAccess(new ApiError(503, 'unavailable'))).toBeNull()
    expect(deniedAccess(new ApiError(403, 'unrecognized'))).toBe('unavailable')
})

it('keeps navigation and refresh available when the queue read fails', async () => {
    vi.mocked(api.readPendingMarkers).mockRejectedValueOnce(new ApiError(503, 'unavailable'))
    mount()
    await screen.findByText('The request failed. Refresh and try again.')
    expect(screen.getByRole('link', { name: 'All places' })).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }))
    await screen.findByRole('heading', { name: 'Review synthetic' })
    expect(api.moderate).not.toHaveBeenCalled()
})

it('loads only the selected edit baseline and keeps it actionable', async () => {
    vi.mocked(api.readPendingMarkers).mockResolvedValue([])
    const places = [
        syntheticPlace({ id: 10, title: '中文原文一', contentLanguage: 'zh' }),
        syntheticPlace({ id: 20, title: '中文原文二', contentLanguage: 'zh' }),
    ]
    const proposals = places.map((place, index) => ({
        ...place,
        id: index + 1,
        markerId: place.id,
        markerTitle: place.title,
        title: `中文修改${index}`,
        language: 'zh',
        proposerUsername: 'synthetic',
        proposerPublicId: 'A',
        proposerIsOwner: true,
        status: 'PENDING' as const,
    }))
    vi.mocked(api.readEdits).mockResolvedValue(proposals)
    const read = vi
        .spyOn(privatePlaces, 'readAccountPlace')
        .mockImplementation(async (id) => places.find((place) => String(place.id) === id)!)
    mount('/admin?item=edit-2')
    await screen.findByRole('heading', { name: '中文原文二' })
    expect(read.mock.calls.filter(([id]) => id === '20')).toHaveLength(1)
    expect(read.mock.calls.filter(([id]) => id === '10')).toHaveLength(0)
    await waitFor(() => expect(screen.getByRole('button', { name: 'Approve' })).toBeEnabled())
})
