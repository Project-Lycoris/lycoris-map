import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter } from 'react-router'
import { SessionProvider } from '@/features/auth/SessionProvider'
import { AccountFlowProvider } from '@/features/auth/AccountFlow'
import { LanguageProvider } from '@/shared/i18n'
import * as sessionApi from '@/shared/api/session'
import { ApiError } from '@/shared/api/ApiError'
import type { User } from '@/shared/api/users'
import type { Marker } from '@/shared/api/markers'
import { syntheticPlace } from '@/features/dev/placeFixtures'
import * as api from './api'
import * as privatePlaces from '@/shared/api/privatePlaces'
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

function pendingMarker(overrides: Partial<Marker> = {}): api.ReviewMarker {
    return {
        ...syntheticPlace({
            id: 1,
            title: 'Pending place',
            username: 'alice',
            reviewStatus: 'PENDING',
            createdAt: '2026-10-18T06:50:15Z',
            ...overrides,
        }),
        proposerSubmissionStats: { approved: 4, rejected: 2, pending: 1 },
    }
}

function editProposal(overrides: Partial<api.EditProposal> = {}): api.EditProposal {
    return {
        id: 40,
        markerId: 9,
        markerTitle: 'Current place title',
        lat: 31.2,
        lng: 121.4,
        category: 'accessible_toilet',
        categories: ['accessible_toilet'],
        openingHoursNote: 'Ring the bell.',
        venueType: 'mall',
        title: 'Proposed title',
        description: 'Proposed description',
        language: 'zh',
        isPublic: true,
        isActive: true,
        openTimeStart: '08:00',
        openTimeEnd: '22:00',
        proposerUsername: 'bob',
        proposerPublicId: 'B',
        proposerIsOwner: true,
        status: 'PENDING',
        createdAt: '2026-10-18T07:50:15Z',
        proposerSubmissionStats: { approved: 1, rejected: 0, pending: 2 },
        ...overrides,
    }
}

function imageProposal(overrides: Partial<api.ImageProposal> = {}): api.ImageProposal {
    return {
        id: 5,
        markerId: 9,
        markerTitle: 'Photo place',
        proposerUsername: 'carol',
        proposerPublicId: 'C',
        imageUrl: '/uploads/markers/photo.jpg',
        status: 'PENDING',
        createdAt: '2026-10-18T08:50:15Z',
        ...overrides,
    }
}

let client: QueryClient

function mount(path = '/admin', language: 'en' | 'zh' = 'en') {
    client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
    return render(
        <MemoryRouter initialEntries={[path]}>
            <QueryClientProvider client={client}>
                <LanguageProvider initialPreference={language}>
                    <SessionProvider>
                        <AccountFlowProvider>
                            <AdminPage />
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
    vi.spyOn(api, 'readMarkers').mockResolvedValue([])
    vi.spyOn(api, 'readPendingMarkers').mockResolvedValue([pendingMarker()])
    vi.spyOn(api, 'readEdits').mockResolvedValue([])
    vi.spyOn(api, 'readImages').mockResolvedValue([])
    vi.spyOn(api, 'moderate').mockResolvedValue(undefined)
    vi.spyOn(api, 'batchModerate').mockResolvedValue({ results: [] })
    vi.spyOn(api, 'readModerationEvents').mockResolvedValue({
        page: 0,
        size: 20,
        totalPages: 1,
        totalElements: 0,
        items: [],
    })
    vi.spyOn(privatePlaces, 'readAccountPlace').mockResolvedValue(
        syntheticPlace({ id: 9, title: 'Current place title' }),
    )
})

afterEach(() => {
    cleanup()
    client?.clear()
    vi.restoreAllMocks()
    vi.useRealTimers()
    vi.unstubAllGlobals()
})

async function ready(path = '/admin') {
    const view = mount(path)
    await screen.findByRole('heading', { name: 'Pending place' })
    return view
}

it('removes an approved item immediately, undoes without a request, then sends after five seconds', async () => {
    await ready()
    fireEvent.click(screen.getByRole('button', { name: 'Approve' }))
    expect(screen.queryByRole('heading', { name: 'Pending place' })).not.toBeInTheDocument()
    expect(screen.getByText(/^Approved ·/)).toBeInTheDocument()
    expect(api.moderate).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Undo' }))
    await screen.findByRole('heading', { name: 'Pending place' })
    expect(api.moderate).not.toHaveBeenCalled()

    vi.useFakeTimers()
    fireEvent.click(screen.getByRole('button', { name: 'Approve' }))
    await act(async () => {
        await vi.advanceTimersByTimeAsync(5000)
    })
    expect(api.moderate).toHaveBeenCalledTimes(1)
    expect(vi.mocked(api.moderate).mock.calls[0]!.slice(0, 4)).toEqual([
        'markers',
        1,
        'approve',
        'zh',
    ])
})

it('flushes pending decisions on pagehide with keepalive', async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response(null, { status: 204 }))
    vi.stubGlobal('fetch', fetcher)
    await ready()
    fireEvent.click(screen.getByRole('button', { name: 'Approve' }))
    fireEvent(window, new Event('pagehide'))
    expect(fetcher).toHaveBeenCalledWith(
        '/api/admin/markers/1/approve?lang=zh',
        expect.objectContaining({ method: 'POST', credentials: 'include', keepalive: true }),
    )
})

it('restores an item and explains a failed decision', async () => {
    vi.mocked(api.moderate).mockRejectedValue(new ApiError(409, 'conflict'))
    await ready()
    vi.useFakeTimers()
    fireEvent.click(screen.getByRole('button', { name: 'Approve' }))
    await act(async () => {
        await vi.advanceTimersByTimeAsync(5000)
    })
    expect(screen.getByRole('heading', { name: 'Pending place' })).toBeInTheDocument()
    expect(screen.getByRole('alert')).toHaveTextContent(
        'The item changed. Review the refreshed list.',
    )
})

it('sends the quick reason and free text when rejecting, and allows no reason', async () => {
    await ready()
    vi.useFakeTimers()
    fireEvent.click(screen.getByRole('button', { name: 'Reject' }))
    const panel = screen.getByRole('region', { name: 'Reject reason' })
    fireEvent.click(within(panel).getByRole('button', { name: 'Information is inaccurate' }))
    fireEvent.change(within(panel).getByLabelText('Reason (optional)'), {
        target: { value: 'typo in the address' },
    })
    fireEvent.click(within(panel).getByRole('button', { name: 'Confirm reject' }))
    await act(async () => {
        await vi.advanceTimersByTimeAsync(5000)
    })
    expect(api.moderate).toHaveBeenLastCalledWith(
        'markers',
        1,
        'reject',
        'zh',
        expect.any(AbortSignal),
        { reason: 'Information is inaccurate · typo in the address' },
    )
})

it('approves with changes and sends the changes payload', async () => {
    await ready()
    fireEvent.click(screen.getByRole('button', { name: 'Modify and approve' }))
    fireEvent.change(screen.getByLabelText('Title'), { target: { value: 'Corrected title' } })
    fireEvent.click(screen.getByRole('button', { name: 'Approve with changes' }))
    await waitFor(() => expect(api.moderate).toHaveBeenCalledTimes(1))
    expect(api.moderate).toHaveBeenCalledWith(
        'markers',
        1,
        'approve',
        'zh',
        expect.any(AbortSignal),
        expect.objectContaining({ changes: expect.objectContaining({ title: 'Corrected title' }) }),
    )
    await waitFor(() =>
        expect(screen.queryByRole('heading', { name: 'Pending place' })).not.toBeInTheDocument(),
    )
})

it('batches three approvals and restores only the failed result', async () => {
    const one = pendingMarker({ id: 1, title: 'One' })
    const two = pendingMarker({ id: 2, title: 'Two' })
    const three = pendingMarker({ id: 3, title: 'Three' })
    vi.mocked(api.readPendingMarkers).mockResolvedValue([one, two, three])
    vi.mocked(api.batchModerate).mockResolvedValue({
        results: [
            { kind: 'marker', id: 1, ok: true },
            { kind: 'marker', id: 2, ok: true },
            { kind: 'marker', id: 3, ok: false, status: 409, message: 'already reviewed' },
        ],
    })
    mount()
    await screen.findByRole('heading', { name: 'One' })
    for (const title of ['One', 'Two', 'Three'])
        fireEvent.click(screen.getByRole('checkbox', { name: `Select ${title}` }))
    const bulk = screen.getByRole('region', { name: 'Bulk actions' })
    vi.useFakeTimers()
    fireEvent.click(within(bulk).getByRole('button', { name: 'Approve' }))
    expect(screen.queryByText('One')).not.toBeInTheDocument()
    await act(async () => {
        await vi.advanceTimersByTimeAsync(5000)
    })
    expect(api.batchModerate).toHaveBeenCalledTimes(1)
    expect(vi.mocked(api.batchModerate).mock.calls[0]![0]).toHaveLength(3)
    expect(screen.getAllByText('Three').length).toBeGreaterThan(0)
    expect(screen.getByRole('alert')).toHaveTextContent('Succeeded 2 items, Failed 1 items')
})

it('counts types, filters and searches the queue', async () => {
    vi.mocked(api.readPendingMarkers).mockResolvedValue([
        pendingMarker({ id: 1, title: 'Alpha place', username: 'alice' }),
    ])
    vi.mocked(api.readEdits).mockResolvedValue([
        editProposal({ id: 40, markerTitle: 'Beta current', title: 'Beta proposal' }),
        editProposal({
            id: 41,
            markerId: 10,
            markerTitle: 'Gamma current',
            proposerUsername: 'dave',
        }),
    ])
    vi.mocked(api.readImages).mockResolvedValue([imageProposal()])
    mount()
    await screen.findByRole('heading', { name: 'Alpha place' })
    expect(screen.getByRole('button', { name: 'All (4)' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'New place (1)' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Content changes (2)' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Photo submission (1)' })).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Content changes (2)' }))
    expect(screen.queryByText('Alpha place')).not.toBeInTheDocument()
    expect(screen.getAllByText('Beta current').length).toBeGreaterThan(0)
    fireEvent.change(screen.getByLabelText('Search'), { target: { value: 'dave' } })
    expect(screen.queryAllByText('Beta current')).toHaveLength(0)
    expect(screen.getAllByText('Gamma current').length).toBeGreaterThan(0)
})

it('restores the selected item from the URL and shows the current title', async () => {
    vi.mocked(api.readPendingMarkers).mockResolvedValue([])
    vi.mocked(api.readEdits).mockResolvedValue([editProposal()])
    mount('/admin?item=edit-40')
    await screen.findByRole('heading', { name: 'Current place title' })
    expect(screen.getByRole('button', { name: /Current place title/ })).toHaveAttribute(
        'aria-current',
        'true',
    )
})

it('only highlights changed fields in the comparison table', async () => {
    vi.mocked(api.readPendingMarkers).mockResolvedValue([])
    vi.mocked(api.readEdits).mockResolvedValue([
        editProposal({
            title: 'Proposed title',
            description: 'Synthetic description.',
            venueType: null,
            openingHoursNote: '',
            language: 'en',
        }),
    ])
    mount('/admin?item=edit-40')
    const table = await screen.findByRole('table', { name: 'Proposed changes' })
    expect(within(table).getByRole('row', { name: /Title/ })).toBeInTheDocument()
    expect(within(table).queryByRole('row', { name: /Description/ })).not.toBeInTheDocument()
    expect(screen.getByText(/other fields unchanged/)).toBeInTheDocument()
    expect(screen.getAllByText('Current place title').length).toBeGreaterThan(0)
})

it('supports J/K/A/Z and ignores shortcuts while typing', async () => {
    vi.mocked(api.readPendingMarkers).mockResolvedValue([
        pendingMarker({ id: 1, title: 'One' }),
        pendingMarker({ id: 2, title: 'Two' }),
        pendingMarker({ id: 3, title: 'Three' }),
    ])
    mount()
    await screen.findByRole('heading', { name: 'One' })
    fireEvent.keyDown(window, { key: 'j' })
    await screen.findByRole('heading', { name: 'Two' })
    fireEvent.keyDown(window, { key: 'k' })
    await screen.findByRole('heading', { name: 'One' })
    fireEvent.keyDown(window, { key: 'a' })
    expect(screen.queryByRole('heading', { name: 'One' })).not.toBeInTheDocument()
    fireEvent.keyDown(window, { key: 'z' })
    await screen.findByRole('heading', { name: 'One' })

    fireEvent.keyDown(window, { key: 'j' })
    await screen.findByRole('heading', { name: 'Two' })
    const search = screen.getByLabelText('Search')
    act(() => search.focus())
    fireEvent.keyDown(search, { key: 'j' })
    expect(screen.getByRole('heading', { name: 'Two' })).toBeInTheDocument()
})

it('does not scroll the page when acting on a queue item', async () => {
    const scrollTo = vi.spyOn(window, 'scrollTo').mockImplementation(() => undefined)
    await ready()
    const before = window.scrollY
    fireEvent.click(screen.getByRole('button', { name: 'Approve' }))
    expect(window.scrollY).toBe(before)
    expect(scrollTo).not.toHaveBeenCalled()
})

it('allows rejecting without a reason', async () => {
    await ready()
    vi.useFakeTimers()
    fireEvent.click(screen.getByRole('button', { name: 'Reject' }))
    fireEvent.click(screen.getByRole('button', { name: 'Confirm reject' }))
    await act(async () => {
        await vi.advanceTimersByTimeAsync(5000)
    })
    expect(api.moderate).toHaveBeenCalledTimes(1)
    expect(vi.mocked(api.moderate).mock.calls[0]![0]).toBe('markers')
    expect(vi.mocked(api.moderate).mock.calls[0]![2]).toBe('reject')
})

it('moves focus into the reject panel and back to its trigger', async () => {
    await ready()
    const trigger = screen.getByRole('button', { name: 'Reject' })
    act(() => trigger.focus())
    fireEvent.click(trigger)
    const panel = screen.getByRole('region', { name: 'Reject reason' })
    expect(within(panel).getByRole('button', { name: 'Information is inaccurate' })).toHaveFocus()
    fireEvent.keyDown(window, { key: 'Escape' })
    await waitFor(() => expect(trigger).toHaveFocus())
    expect(screen.queryByRole('region', { name: 'Reject reason' })).not.toBeInTheDocument()
})

it('sends a shared reason for a batch rejection', async () => {
    const one = pendingMarker({ id: 1, title: 'One' })
    const two = pendingMarker({ id: 2, title: 'Two' })
    vi.mocked(api.readPendingMarkers).mockResolvedValue([one, two])
    vi.mocked(api.batchModerate).mockResolvedValue({
        results: [
            { kind: 'marker', id: 1, ok: true },
            { kind: 'marker', id: 2, ok: true },
        ],
    })
    mount()
    await screen.findByRole('heading', { name: 'One' })
    for (const title of ['One', 'Two'])
        fireEvent.click(screen.getByRole('checkbox', { name: `Select ${title}` }))
    const bulk = screen.getByRole('region', { name: 'Bulk actions' })
    fireEvent.click(within(bulk).getByRole('button', { name: 'Reject' }))
    fireEvent.change(within(bulk).getByLabelText('Shared reason (optional)'), {
        target: { value: 'Duplicate place' },
    })
    vi.useFakeTimers()
    fireEvent.click(within(bulk).getByRole('button', { name: 'Confirm reject' }))
    await act(async () => {
        await vi.advanceTimersByTimeAsync(5000)
    })
    expect(api.batchModerate).toHaveBeenCalledWith(
        [
            { kind: 'marker', id: 1, decision: 'reject', reason: 'Duplicate place' },
            { kind: 'marker', id: 2, decision: 'reject', reason: 'Duplicate place' },
        ],
        expect.any(AbortSignal),
    )
})

it('loads the whole queue in one list without pagination', async () => {
    vi.mocked(api.readPendingMarkers).mockResolvedValue(
        Array.from({ length: 240 }, (_, index) =>
            pendingMarker({ id: index + 1, title: `Queued ${index}` }),
        ),
    )
    mount()
    await screen.findByRole('heading', { name: 'Queued 0' })
    expect(screen.getAllByRole('button', { name: /Queued / })).toHaveLength(240)
    expect(screen.queryByRole('button', { name: 'Next' })).not.toBeInTheDocument()
})

it('suspends shortcuts while the reason form is open and restores them after', async () => {
    vi.mocked(api.readPendingMarkers).mockResolvedValue([
        pendingMarker({ id: 1, title: 'One' }),
        pendingMarker({ id: 2, title: 'Two' }),
    ])
    mount()
    await screen.findByRole('heading', { name: 'One' })
    fireEvent.click(screen.getByRole('button', { name: 'Reject' }))
    screen.getByRole('region', { name: 'Reject reason' })
    fireEvent.keyDown(window, { key: 'a' })
    fireEvent.keyDown(window, { key: 'j' })
    fireEvent.keyDown(window, { key: 'z' })
    expect(screen.getByRole('heading', { name: 'One' })).toBeInTheDocument()
    expect(api.moderate).not.toHaveBeenCalled()
    fireEvent.keyDown(window, { key: 'Escape' })
    await waitFor(() =>
        expect(screen.queryByRole('region', { name: 'Reject reason' })).not.toBeInTheDocument(),
    )
    fireEvent.keyDown(window, { key: 'j' })
    await screen.findByRole('heading', { name: 'Two' })
})

it('flags a stale edit proposal, shows the current title, and blocks approve/modify', async () => {
    vi.mocked(api.readPendingMarkers).mockResolvedValue([])
    vi.mocked(api.readEdits).mockResolvedValue([editProposal({ baseMarkerVersion: 1 })])
    vi.mocked(privatePlaces.readAccountPlace).mockResolvedValue(
        syntheticPlace({ id: 9, title: 'Fresh title', version: 2 }),
    )
    mount('/admin?item=edit-40')
    await screen.findByRole('heading', { name: 'Fresh title' })
    expect(screen.queryByRole('heading', { name: 'Proposed title' })).not.toBeInTheDocument()
    await screen.findByText(
        'This place changed after the proposal was submitted. The proposal is out of date and can only be rejected.',
    )
    await waitFor(() => expect(screen.getByRole('button', { name: 'Approve' })).toBeDisabled())
    expect(screen.getByRole('button', { name: 'Modify and approve' })).toBeDisabled()
    expect(screen.getAllByText('Out of date').length).toBeGreaterThan(0)
    fireEvent.keyDown(window, { key: 'a' })
    fireEvent.keyDown(window, { key: 'e' })
    expect(screen.getByRole('heading', { name: 'Fresh title' })).toBeInTheDocument()
    expect(api.moderate).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Reject' }))
    expect(screen.getByRole('button', { name: /Proposal is out of date/ })).toHaveAttribute(
        'aria-pressed',
        'true',
    )
})

it('does not flag proposals when the base version matches or is unknown', async () => {
    vi.mocked(api.readPendingMarkers).mockResolvedValue([])
    vi.mocked(api.readEdits).mockResolvedValue([
        editProposal({ id: 40, markerId: 9, baseMarkerVersion: 2 }),
        editProposal({
            id: 41,
            markerId: 10,
            markerTitle: 'Legacy proposal',
            baseMarkerVersion: null,
        }),
    ])
    vi.mocked(privatePlaces.readAccountPlace).mockImplementation(async (value) =>
        syntheticPlace({ id: Number(value), title: `Place ${value}`, version: 2 }),
    )
    mount('/admin?item=edit-40')
    await screen.findByRole('heading', { name: 'Place 9' })
    await waitFor(() => expect(screen.getByRole('button', { name: 'Approve' })).toBeEnabled())
    expect(screen.queryByText('Out of date')).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: /Legacy proposal/ }))
    await screen.findByRole('heading', { name: 'Place 10' })
    expect(screen.queryByText('Out of date')).not.toBeInTheDocument()
})

it('skips out-of-date proposals during a bulk approval and reports how many', async () => {
    vi.mocked(api.readPendingMarkers).mockResolvedValue([])
    const stale = editProposal({
        id: 40,
        markerId: 9,
        markerTitle: 'Stale place',
        baseMarkerVersion: 1,
        createdAt: '2026-10-18T06:00:00Z',
    })
    const fresh = editProposal({
        id: 41,
        markerId: 10,
        markerTitle: 'Fresh place',
        baseMarkerVersion: 2,
        createdAt: '2026-10-18T07:00:00Z',
    })
    vi.mocked(api.readEdits).mockResolvedValue([stale, fresh])
    vi.mocked(privatePlaces.readAccountPlace).mockImplementation(async (value) =>
        syntheticPlace({ id: Number(value), title: `Place ${value}`, version: 2 }),
    )
    vi.mocked(api.batchModerate).mockResolvedValue({
        results: [{ kind: 'edit', id: 41, ok: true }],
    })
    mount()
    await screen.findByRole('heading', { name: 'Place 9' })
    await waitFor(() => expect(screen.getAllByText('Out of date')).toHaveLength(1))
    fireEvent.click(screen.getByRole('checkbox', { name: 'Select Stale place' }))
    fireEvent.click(screen.getByRole('checkbox', { name: 'Select Fresh place' }))
    const bulk = screen.getByRole('region', { name: 'Bulk actions' })
    vi.useFakeTimers()
    fireEvent.click(within(bulk).getByRole('button', { name: 'Approve' }))
    await act(async () => {
        await vi.advanceTimersByTimeAsync(5000)
    })
    expect(api.batchModerate).toHaveBeenCalledTimes(1)
    expect(vi.mocked(api.batchModerate).mock.calls[0]![0]).toEqual([
        { kind: 'edit', id: 41, decision: 'approve' },
    ])
    expect(screen.getByText(/Skipped 1/)).toBeInTheDocument()
})

it('translates stored shortcut reasons in the moderation timeline', async () => {
    vi.mocked(api.readPendingMarkers).mockResolvedValue([])
    vi.mocked(api.readEdits).mockResolvedValue([editProposal()])
    vi.mocked(api.readModerationEvents).mockResolvedValue({
        page: 0,
        size: 20,
        totalPages: 1,
        totalElements: 1,
        items: [
            {
                id: 1,
                createdAt: '2026-10-18T06:50:15Z',
                actorUsername: 'erin',
                actorPublicId: 'E',
                action: 'reject',
                targetKind: 'edit',
                targetId: 40,
                markerId: 9,
                markerTitle: 'Current place title',
                targetLabel: null,
                note: 'Information is inaccurate · typo',
            },
        ],
    })
    mount('/admin?item=edit-40', 'zh')
    const timeline = await screen.findByRole('region', { name: '审核记录' })
    expect(await within(timeline).findByText('信息不准确 · typo')).toBeInTheDocument()
})
