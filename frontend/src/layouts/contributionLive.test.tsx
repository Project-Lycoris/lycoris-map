import { BrowserDraftJournal } from '@/features/contributions/DraftJournal'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router'
import { afterEach, expect, it, vi } from 'vitest'
import { AppProviders } from '@/app/providers'
import { MapPage } from '@/app/MapPage'
import { useContributions } from '@/features/contributions/ContributionsProvider'
import { useSessionStore } from '@/features/auth/SessionProvider'
import type { ContributionStore } from '@/features/contributions/ContributionStore'
import * as writes from '@/shared/api/markerWrites'
import { syntheticPlace } from '@/features/dev/placeFixtures'
afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
})
function mount(clusterSize = 0) {
    vi.spyOn(BrowserDraftJournal.prototype, 'list').mockResolvedValue([])
    vi.spyOn(BrowserDraftJournal.prototype, 'put').mockResolvedValue(undefined)
    vi.spyOn(BrowserDraftJournal.prototype, 'remove').mockResolvedValue(undefined)
    let resetSession = () => {}
    let mobile = false,
        store: ContributionStore | null = null
    const listeners = new Set<() => void>()
    vi.stubGlobal('matchMedia', () => ({
        get matches() {
            return mobile
        },
        addEventListener: (_: string, fn: () => void) => listeners.add(fn),
        removeEventListener: (_: string, fn: () => void) => listeners.delete(fn),
    }))
    vi.stubGlobal(
        'fetch',
        vi.fn(async (input: RequestInfo | URL) => {
            const url = String(input)
            return new Response(
                JSON.stringify(
                    url.includes('/api/me')
                        ? {
                              code: 0,
                              message: 'ok',
                              data: {
                                  publicId: 'A',
                                  username: 'Synthetic',
                                  nickname: null,
                                  email: null,
                                  avatarUrl: null,
                                  pronouns: null,
                                  signature: null,
                              },
                          }
                        : url.includes('/api/markers/viewport')
                          ? Array.from({ length: clusterSize }, (_, index) =>
                                syntheticPlace({ id: index + 1, lat: 31.2304, lng: 121.4737 }),
                            )
                          : [],
                ),
                { headers: { 'Content-Type': 'application/json' } },
            )
        }),
    )
    function Probe() {
        store = useContributions().store
        resetSession = useSessionStore()!.externalChange
        return null
    }
    const view = render(
        <MemoryRouter initialEntries={['/?lang=en']}>
            <AppProviders>
                <MapPage />
                <Probe />
            </AppProviders>
        </MemoryRouter>,
    )
    return {
        ...view,
        getStore: () => store!,
        resetSession: () => act(resetSession),
        phone: () =>
            act(() => {
                mobile = true
                listeners.forEach((fn) => fn())
            }),
    }
}
it('does not choose the map center when a desktop picker becomes a phone picker', async () => {
    const view = mount()
    await waitFor(() => expect(view.getStore().getSnapshot().owner).not.toBeNull())
    const map = view.container.querySelector('.leaflet-container')!
    fireEvent.click(screen.getByRole('button', { name: 'Contribute' }))
    view.phone()
    expect(view.getStore().getSnapshot().point).toBeNull()
    expect(screen.queryByRole('form', { name: 'Contribution draft' })).not.toBeInTheDocument()
    expect(screen.getByText('Click on the map to add points.')).toBeInTheDocument()
    expect(view.container.querySelector('.leaflet-container')).toBe(map)
    fireEvent.click(map, { clientX: 180, clientY: 260 })
    await screen.findByRole('form', { name: 'Contribution draft' })
    expect(view.getStore().getSnapshot().point).not.toBeNull()
})
it('submits the phone map click coordinates and preserves them in the save receipt', async () => {
    const view = mount(),
        store = view.getStore()
    await waitFor(() => expect(store.getSnapshot().owner).not.toBeNull())
    const write = vi
        .spyOn(writes, 'createMarker')
        .mockImplementation(async (payload) =>
            syntheticPlace({ ...payload, reviewStatus: 'PENDING' }),
        )
    view.phone()
    fireEvent.click(screen.getByRole('button', { name: 'Contribute' }))
    expect(store.getSnapshot().point).toBeNull()
    fireEvent.click(view.container.querySelector('.product-map')!, { clientX: 190, clientY: 290 })
    const selected = store.getSnapshot().point!
    expect(selected).not.toEqual({ lat: 31.2304, lng: 121.4737 })
    fireEvent.change(screen.getByRole('textbox', { name: 'Title' }), {
        target: { value: '所选位置' },
    })
    fireEvent.click(screen.getByRole('checkbox', { name: 'Accessible Toilets' }))
    fireEvent.click(screen.getByRole('button', { name: 'Submit' }))
    await waitFor(() => expect(write).toHaveBeenCalledTimes(1))
    expect(write.mock.calls[0]![0]).toMatchObject({ ...selected, title: '所选位置' })
    await screen.findByRole('button', { name: 'View place' })
    expect(store.getSnapshot().saved).toMatchObject(selected)
})
it('keeps map selection active when tapping an overlapping marker cluster', async () => {
    const view = mount(12)
    await waitFor(() => expect(view.getStore().getSnapshot().owner).not.toBeNull())
    const cluster = await screen.findByRole('button', { name: '12 places' })
    view.phone()
    fireEvent.click(screen.getByRole('button', { name: 'Contribute' }))
    fireEvent.click(cluster)
    expect(screen.getByText('Click on the map to add points.')).toBeInTheDocument()
    expect(screen.queryByRole('form', { name: 'Contribution draft' })).not.toBeInTheDocument()
    expect(view.getStore().getSnapshot().point).toBeNull()
})
it('reopens the same submitting form from Contribute, without creating another request', async () => {
    const view = mount(),
        store = view.getStore()
    await waitFor(() => expect(store.getSnapshot().owner).not.toBeNull())
    let finish!: (value: ReturnType<typeof syntheticPlace>) => void
    const write = vi.spyOn(writes, 'createMarker').mockImplementation(
        () =>
            new Promise((resolve) => {
                finish = resolve
            }),
    )
    fireEvent.click(screen.getByRole('button', { name: 'Contribute' }))
    fireEvent.click(view.container.querySelector('.product-map')!, { clientX: 650, clientY: 350 })
    fireEvent.change(screen.getByRole('textbox', { name: 'Title' }), {
        target: { value: 'Synthetic' },
    })
    fireEvent.click(screen.getByRole('checkbox', { name: 'Accessible Toilets' }))
    fireEvent.click(screen.getByRole('button', { name: 'Submit' }))
    await waitFor(() => expect(write).toHaveBeenCalledTimes(1))
    fireEvent.click(screen.getByRole('button', { name: 'Close contribution form' }))
    fireEvent.click(screen.getByRole('button', { name: 'Contribute' }))
    expect(screen.getByRole('textbox', { name: 'Title' })).toHaveValue('Synthetic')
    expect(screen.getByRole('button', { name: 'Saving…' })).toBeDisabled()
    await act(async () => {
        finish(syntheticPlace({ reviewStatus: 'PENDING' }))
    })
    expect(await screen.findByRole('button', { name: 'View place' })).toHaveFocus()
    expect(write).toHaveBeenCalledTimes(1)
})
it('returns to map selection when an account change clears the selected point', async () => {
    const view = mount()
    await waitFor(() => expect(view.getStore().getSnapshot().owner).not.toBeNull())
    view.phone()
    fireEvent.click(screen.getByRole('button', { name: 'Contribute' }))
    fireEvent.click(view.container.querySelector('.product-map')!, { clientX: 180, clientY: 260 })
    fireEvent.change(screen.getByRole('textbox', { name: 'Title' }), {
        target: { value: 'Old private draft' },
    })
    view.resetSession()
    await waitFor(() =>
        expect(screen.getByText('Click on the map to add points.')).toBeInTheDocument(),
    )
    expect(screen.queryByRole('form', { name: 'Contribution draft' })).not.toBeInTheDocument()
    expect(view.getStore().getSnapshot().point).toBeNull()
    expect(view.getStore().getSnapshot().draft.photo).toBeNull()
})
