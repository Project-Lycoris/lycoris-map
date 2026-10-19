import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter, useLocation } from 'react-router'
import { SessionProvider } from '@/features/auth/SessionProvider'
import { AccountFlowProvider } from '@/features/auth/AccountFlow'
import { LanguageProvider } from '@/shared/i18n'
import * as sessionApi from '@/shared/api/session'
import { ApiError } from '@/shared/api/ApiError'
import type { User } from '@/shared/api/users'
import { syntheticPlace } from '@/features/dev/placeFixtures'
import * as api from './api'
import AdminPage from './AdminPage'

const user: User = {
    publicId: 'A',
    username: 'admin',
    email: null,
    nickname: null,
    avatarUrl: null,
    pronouns: null,
    signature: null,
}

function adminMarker(overrides: Partial<api.AdminMarker> = {}): api.AdminMarker {
    return {
        ...syntheticPlace({ id: 1, title: 'Place one', reviewStatus: 'APPROVED' }),
        reviewNote: null,
        ...overrides,
    }
}

function page(items: api.AdminMarker[], overrides: Partial<api.AdminMarkerPage> = {}) {
    return {
        page: overrides.page ?? 0,
        size: 20,
        totalPages: overrides.totalPages ?? 1,
        totalElements: overrides.totalElements ?? items.length,
        items,
    }
}

const events: api.ModerationEvent[] = [
    {
        id: 5,
        createdAt: '2026-10-18T06:50:15Z',
        actorUsername: 'erin',
        actorPublicId: 'E',
        action: 'approve_with_changes',
        targetKind: 'edit',
        targetId: 4,
        markerId: 1,
        markerTitle: 'Reviewed place',
        targetLabel: null,
        note: 'Information is inaccurate · fixed a typo',
    },
    {
        id: 4,
        createdAt: '2026-10-17T06:50:15Z',
        actorUsername: 'erin',
        actorPublicId: 'E',
        action: 'user_disable',
        targetKind: 'user',
        targetId: 9,
        markerId: null,
        markerTitle: null,
        targetLabel: 'bob',
        note: null,
    },
]

let client: QueryClient

function LocationProbe() {
    const location = useLocation()
    return <output data-testid="location">{location.pathname + location.search}</output>
}

function mount(path: string, language: 'en' | 'zh' = 'en') {
    client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
    return render(
        <MemoryRouter initialEntries={[path]}>
            <QueryClientProvider client={client}>
                <LanguageProvider initialPreference={language}>
                    <SessionProvider>
                        <AccountFlowProvider>
                            <AdminPage />
                            <LocationProbe />
                        </AccountFlowProvider>
                    </SessionProvider>
                </LanguageProvider>
            </QueryClientProvider>
        </MemoryRouter>,
    )
}

beforeEach(() => {
    vi.spyOn(sessionApi, 'fetchMe').mockResolvedValue(user)
    vi.spyOn(api, 'readUsers').mockResolvedValue({
        page: 0,
        size: 10,
        totalPages: 0,
        totalElements: 0,
        items: [],
    })
    vi.spyOn(api, 'readPendingMarkers').mockResolvedValue([])
    vi.spyOn(api, 'readEdits').mockResolvedValue([])
    vi.spyOn(api, 'readImages').mockResolvedValue([])
    vi.spyOn(api, 'searchMarkers').mockResolvedValue(page([adminMarker()]))
    vi.spyOn(api, 'readModerationEvents').mockResolvedValue({
        page: 0,
        size: 20,
        totalPages: 1,
        totalElements: 0,
        items: [],
    })
    vi.spyOn(api, 'moderate').mockResolvedValue(undefined)
    vi.spyOn(api, 'deactivateMarker').mockResolvedValue(undefined)
    vi.spyOn(api, 'restoreMarker').mockResolvedValue(undefined)
    vi.spyOn(api, 'editMarker').mockResolvedValue(syntheticPlace())
})

afterEach(() => {
    cleanup()
    client?.clear()
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
    vi.useRealTimers()
})

it('requests fresh results whenever only the search text changes', async () => {
    vi.mocked(api.searchMarkers).mockImplementation(async ({ q }) =>
        page([adminMarker({ title: q ? `Result for ${q}` : 'Place one' })]),
    )
    mount('/admin/all')
    await screen.findByRole('button', { name: /Place one/ })
    fireEvent.change(screen.getByLabelText('Search'), { target: { value: 'airport' } })
    await screen.findByRole('button', { name: /Result for airport/ })
    fireEvent.change(screen.getByLabelText('Search'), { target: { value: 'park' } })
    await screen.findByRole('button', { name: /Result for park/ })
    expect(vi.mocked(api.searchMarkers).mock.calls.at(-1)![0].q).toBe('park')
})

it('refreshes disable and restore state after each successful action', async () => {
    let disabled = false
    vi.mocked(api.searchMarkers).mockImplementation(async () =>
        page([adminMarker({ deactivated: disabled })]),
    )
    vi.mocked(api.deactivateMarker).mockImplementation(async () => {
        disabled = true
    })
    vi.mocked(api.restoreMarker).mockImplementation(async () => {
        disabled = false
    })
    mount('/admin/all?markerId=1&q=1')
    fireEvent.click(await screen.findByRole('button', { name: 'Disable' }))
    fireEvent.click(
        within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Confirm' }),
    )
    fireEvent.click(await screen.findByRole('button', { name: 'Restore' }))
    fireEvent.click(
        within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Confirm' }),
    )
    await screen.findByRole('button', { name: 'Disable' })
    expect(api.restoreMarker).toHaveBeenCalledTimes(1)
})

it('keeps a reviewed place visible with its updated status in the all-places list', async () => {
    let approved = false
    vi.mocked(api.searchMarkers).mockImplementation(async () =>
        page([adminMarker({ reviewStatus: approved ? 'APPROVED' : 'PENDING' })]),
    )
    vi.mocked(api.moderate).mockImplementation(async () => {
        approved = true
    })
    mount('/admin/all?markerId=1&q=1')
    await screen.findByRole('button', { name: 'Approve' })
    vi.useFakeTimers()
    fireEvent.click(screen.getByRole('button', { name: 'Approve' }))
    await act(async () => {
        await vi.advanceTimersByTimeAsync(5000)
    })
    vi.useRealTimers()
    await screen.findByRole('heading', { name: 'Place one' })
    expect(screen.getByRole('button', { name: /Place one.*Approved/ })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Approve' })).not.toBeInTheDocument()
})

it('sends the URL filters to the server and updates them from the controls', async () => {
    vi.mocked(api.searchMarkers).mockResolvedValue(page([adminMarker()]))
    mount('/admin/all?status=PENDING&category=baby_room&deactivated=false&sort=created_asc&page=1')
    await waitFor(() => expect(api.searchMarkers).toHaveBeenCalled())
    expect(vi.mocked(api.searchMarkers).mock.calls[0]![0]).toEqual(
        expect.objectContaining({
            page: 1,
            status: 'PENDING',
            category: 'baby_room',
            deactivated: false,
            sort: 'created_asc',
        }),
    )
    fireEvent.change(screen.getByLabelText('Review status'), { target: { value: 'APPROVED' } })
    await waitFor(() =>
        expect(vi.mocked(api.searchMarkers).mock.calls.at(-1)![0]).toEqual(
            expect.objectContaining({ page: 0, status: 'APPROVED' }),
        ),
    )
    expect(screen.getByTestId('location')).toHaveTextContent('status=APPROVED')
})

it('shows approve and reject only for pending places', async () => {
    const pending = adminMarker({ id: 1, title: 'Waiting', reviewStatus: 'PENDING' })
    const approved = adminMarker({ id: 2, title: 'Live', reviewStatus: 'APPROVED' })
    vi.mocked(api.searchMarkers).mockResolvedValue(page([pending, approved]))
    mount('/admin/all?markerId=1')
    await screen.findByRole('heading', { name: 'Waiting' })
    expect(screen.getByRole('button', { name: 'Approve' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Reject' })).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: /Live/ }))
    await screen.findByRole('heading', { name: 'Live' })
    expect(screen.queryByRole('button', { name: 'Approve' })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Reject' })).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Edit' })).toBeInTheDocument()
})

it('edits in place and keeps the same place selected after saving', async () => {
    vi.mocked(api.searchMarkers).mockResolvedValue(
        page([adminMarker({ id: 2, title: 'Editable' })]),
    )
    const scrollTo = vi.spyOn(window, 'scrollTo').mockImplementation(() => undefined)
    mount('/admin/all?markerId=2')
    await screen.findByRole('heading', { name: 'Editable' })
    fireEvent.click(screen.getByRole('button', { name: 'Edit' }))
    fireEvent.change(screen.getByLabelText('Title'), { target: { value: 'Fixed' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    const confirm = screen.getByRole('alertdialog')
    fireEvent.click(within(confirm).getByRole('button', { name: 'Confirm' }))
    await waitFor(() => expect(api.editMarker).toHaveBeenCalledTimes(1))
    await screen.findByRole('heading', { name: 'Editable' })
    expect(scrollTo).not.toHaveBeenCalled()
})

it('confirms disabling in place instead of at the top of the page', async () => {
    vi.mocked(api.searchMarkers).mockResolvedValue(
        page([adminMarker({ id: 2, title: 'Toggle', deactivated: false })]),
    )
    mount('/admin/all?markerId=2')
    await screen.findByRole('heading', { name: 'Toggle' })
    fireEvent.click(screen.getByRole('button', { name: 'Disable' }))
    const dialog = screen.getByRole('alertdialog')
    expect(dialog.closest('.review-detail')).not.toBeNull()
    fireEvent.click(within(dialog).getByRole('button', { name: 'Confirm' }))
    await waitFor(() => expect(api.deactivateMarker).toHaveBeenCalledWith(2, expect.anything()))
})

it('renders the moderation timeline in the detail panel', async () => {
    vi.mocked(api.searchMarkers).mockResolvedValue(
        page([adminMarker({ id: 1, title: 'Timeline' })]),
    )
    vi.mocked(api.readModerationEvents).mockResolvedValue({
        page: 0,
        size: 20,
        totalPages: 1,
        totalElements: 1,
        items: [events[0]!],
    })
    mount('/admin/all?markerId=1')
    await screen.findByRole('heading', { name: 'Timeline' })
    const timeline = await screen.findByRole('region', { name: 'Moderation history' })
    expect(within(timeline).getByText(/erin/)).toBeInTheDocument()
    expect(within(timeline).getByText(/Approved with changes/)).toBeInTheDocument()
    expect(within(timeline).getByText(/fixed a typo/)).toBeInTheDocument()
})

it('degrades the timeline and the all-places list when the routes are missing', async () => {
    vi.mocked(api.readModerationEvents).mockRejectedValue(new ApiError(404, 'not found'))
    vi.mocked(api.searchMarkers).mockRejectedValue(new ApiError(405, 'method not allowed'))
    mount('/admin/all?markerId=1')
    expect(
        await screen.findByText('Server update required to browse all places'),
    ).toBeInTheDocument()
})

it('renders readable event rows instead of raw target ids', async () => {
    vi.mocked(api.readModerationEvents).mockResolvedValue({
        page: 0,
        size: 50,
        totalPages: 1,
        totalElements: 2,
        items: events,
    })
    mount('/admin/log', 'zh')
    await screen.findByText('内容修改 · Reviewed place')
    expect(screen.getByText('账号 · bob')).toBeInTheDocument()
    expect(screen.getByText('信息不准确 · fixed a typo')).toBeInTheDocument()
    expect(screen.queryByText('edit #4')).not.toBeInTheDocument()
    expect(screen.queryByText('user #9')).not.toBeInTheDocument()
})

it('paginates the audit log and links events to their target', async () => {
    vi.mocked(api.readModerationEvents).mockResolvedValue({
        page: 0,
        size: 50,
        totalPages: 2,
        totalElements: 60,
        items: events,
    })
    mount('/admin/log')
    await screen.findByText('Approved with changes')
    expect(screen.getAllByRole('button', { name: /erin/ })).toHaveLength(2)
    expect(screen.getByText('Account disabled')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: /erin.*Approved with changes/s }))
    await waitFor(() =>
        expect(screen.getByTestId('location')).toHaveTextContent('/admin/all?markerId=1'),
    )
})

it('links user events to the user page search and tolerates a missing log route', async () => {
    vi.mocked(api.readModerationEvents).mockResolvedValue({
        page: 0,
        size: 50,
        totalPages: 1,
        totalElements: 1,
        items: [events[1]!],
    })
    mount('/admin/log')
    fireEvent.click(await screen.findByRole('button', { name: /erin/ }))
    await waitFor(() => expect(screen.getByTestId('location')).toHaveTextContent('/admin/usr?q=9'))

    cleanup()
    client.clear()
    vi.mocked(api.readModerationEvents).mockRejectedValue(new ApiError(404, 'not found'))
    mount('/admin/log')
    expect(
        await screen.findByText('Server update required to view moderation history'),
    ).toBeInTheDocument()
})

it('jumps from the audit log to a place search in all places', async () => {
    vi.mocked(api.readModerationEvents).mockResolvedValue({
        page: 0,
        size: 50,
        totalPages: 1,
        totalElements: 1,
        items: [events[0]!],
    })
    mount('/admin/log')
    fireEvent.click(await screen.findByRole('button', { name: /erin/ }))
    await waitFor(() => expect(api.searchMarkers).toHaveBeenCalled())
    expect(screen.getByTestId('location')).toHaveTextContent('/admin/all?markerId=1')
})

it('keeps the current page when using pagination', async () => {
    vi.mocked(api.searchMarkers).mockResolvedValue(
        page([adminMarker({ id: 1, title: 'Paged' })], {
            page: 0,
            totalPages: 2,
            totalElements: 40,
        }),
    )
    mount('/admin/all')
    fireEvent.click(await screen.findByRole('button', { name: /Paged/ }))
    await screen.findByRole('heading', { name: 'Paged' })
    fireEvent.click(screen.getByRole('button', { name: 'Next' }))
    await waitFor(() =>
        expect(vi.mocked(api.searchMarkers).mock.calls.at(-1)![0]).toEqual(
            expect.objectContaining({ page: 1 }),
        ),
    )
    await new Promise((resolve) => setTimeout(resolve, 400))
    expect(vi.mocked(api.searchMarkers).mock.calls.at(-1)![0]).toEqual(
        expect.objectContaining({ page: 1 }),
    )
})

it('does not offer deactivation when the server does not report that state', async () => {
    vi.mocked(api.searchMarkers).mockResolvedValue(
        page([adminMarker({ id: 2, title: 'Legacy', deactivated: undefined })]),
    )
    mount('/admin/all?markerId=2')
    await screen.findByRole('heading', { name: 'Legacy' })
    const waiting = screen.getByRole('button', { name: 'Server update required' })
    expect(waiting).toBeDisabled()
    fireEvent.click(waiting)
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument()
    expect(api.deactivateMarker).not.toHaveBeenCalled()
})

it('has no bulk image-cleanup action in all places', async () => {
    mount('/admin/all')
    await screen.findByRole('button', { name: /Place one/ })
    expect(screen.queryByRole('button', { name: /clean|clear missing/i })).not.toBeInTheDocument()
})
