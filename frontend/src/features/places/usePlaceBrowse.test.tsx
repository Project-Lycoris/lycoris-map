import type { PropsWithChildren } from 'react'
import { act, cleanup, renderHook, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { ApiError } from '@/shared/api/ApiError'
import * as reads from '@/shared/api/markerReads'
import { syntheticPlace } from '@/features/dev/placeFixtures'
import { mapView } from '@/features/map/viewport'
import { usePlaceBrowse } from './usePlaceBrowse'
vi.mock('@/shared/api/markerReads', async (importOriginal) => ({
    ...(await importOriginal<typeof reads>()),
    readViewport: vi.fn(),
    readSearch: vi.fn(),
    readNearby: vi.fn(),
    readPublicPlace: vi.fn(),
}))
const clients: QueryClient[] = []
function wrapper() {
    const client = new QueryClient({
        defaultOptions: { queries: { retry: false, gcTime: Infinity } },
    })
    clients.push(client)
    return ({ children }: PropsWithChildren) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
    )
}
beforeEach(() => {
    vi.mocked(reads.readViewport).mockResolvedValue([syntheticPlace()])
    vi.mocked(reads.readSearch).mockResolvedValue([])
    vi.mocked(reads.readNearby).mockResolvedValue([])
    vi.mocked(reads.readPublicPlace).mockResolvedValue(syntheticPlace())
})
afterEach(() => {
    cleanup()
    clients.splice(0).forEach((client) => client.clear())
    vi.resetAllMocks()
    vi.unstubAllGlobals()
})
it('splits date-line requests without filtering historical categories and deduplicates results', async () => {
    const { result } = renderHook(() => usePlaceBrowse('zh', null), { wrapper: wrapper() })
    act(() => result.current.onView(mapView(-10, 10, 170, 190, { lat: 0, lng: 180 }, 4)))
    await waitFor(() => expect(result.current.results).toHaveLength(1))
    expect(reads.readViewport).toHaveBeenCalledTimes(2)
    expect(reads.readViewport).toHaveBeenCalledWith(
        expect.objectContaining({ minLng: 159, maxLng: 180, categories: [] }),
        'zh',
        expect.any(AbortSignal),
    )
})
it('debounces search, aborts old requests and never lets a late result replace a newer term', async () => {
    let finishOld!: (value: ReturnType<typeof syntheticPlace>[]) => void
    vi.mocked(reads.readSearch).mockImplementation((query) =>
        query === 'old'
            ? new Promise((resolve) => {
                  finishOld = resolve
              })
            : Promise.resolve([syntheticPlace({ title: query })]),
    )
    const { result } = renderHook(() => usePlaceBrowse('en', null), { wrapper: wrapper() })
    act(() => result.current.setSearch('old'))
    expect(reads.readSearch).not.toHaveBeenCalled()
    await waitFor(() => expect(reads.readSearch).toHaveBeenCalledTimes(1))
    const oldSignal = vi.mocked(reads.readSearch).mock.calls[0]![2]
    act(() => result.current.setSearch('new'))
    expect(result.current.state.pending).toBe(true)
    expect(result.current.results).toEqual([])
    await waitFor(() => expect(result.current.results[0]?.title).toBe('new'))
    expect(oldSignal.aborted).toBe(true)
    await act(async () => finishOld([syntheticPlace({ title: 'old' })]))
    expect(result.current.results[0]?.title).toBe('new')
})
it('refetches translated DTOs on language changes without trusting marker version', async () => {
    vi.mocked(reads.readPublicPlace).mockImplementation(async (_, language) =>
        syntheticPlace({ title: language, contentLanguage: language }),
    )
    const { result, rerender } = renderHook(
        ({ lang }: { lang: 'en' | 'zh' }) => usePlaceBrowse(lang, '1'),
        { initialProps: { lang: 'en' }, wrapper: wrapper() },
    )
    await waitFor(() => expect(result.current.detail?.title).toBe('en'))
    rerender({ lang: 'zh' })
    await waitFor(() => expect(result.current.detail?.title).toBe('zh'))
    expect(result.current.detail?.version).toBe(1)
})
it('withdraws a formerly public selected marker and cached list row after a 404', async () => {
    const { result } = renderHook(() => usePlaceBrowse('en', '1'), { wrapper: wrapper() })
    act(() => result.current.onView(mapView(31, 32, 121, 122, { lat: 31.5, lng: 121.5 }, 10)))
    await waitFor(() => expect(result.current.results).toHaveLength(1))
    await waitFor(() => expect(result.current.detail?.id).toBe(1))
    vi.mocked(reads.readPublicPlace).mockRejectedValue(new ApiError(404, 'HTTP 404'))
    act(() => result.current.detailState.retry())
    await waitFor(() => expect(result.current.detailState.error).toBe('This place is unavailable.'))
    expect(result.current.detail).toBeUndefined()
    expect(result.current.markers).toEqual([])
})
it('allows a freshly public list to restore a previously unavailable place', async () => {
    const { result, rerender } = renderHook(
        ({ id }: { id: string | null }) => usePlaceBrowse('en', id),
        { initialProps: { id: '1' as string | null }, wrapper: wrapper() },
    )
    act(() => result.current.onView(mapView(31, 32, 121, 122, { lat: 31.5, lng: 121.5 }, 10)))
    await waitFor(() => expect(result.current.results).toHaveLength(1))
    vi.mocked(reads.readViewport).mockResolvedValue([])
    vi.mocked(reads.readPublicPlace).mockRejectedValue(new ApiError(404, 'HTTP 404'))
    act(() => result.current.detailState.retry())
    await waitFor(() => expect(result.current.detailState.error).toContain('unavailable'))
    await waitFor(() => expect(result.current.results).toEqual([]))
    rerender({ id: null })
    vi.mocked(reads.readViewport).mockResolvedValue([syntheticPlace({ title: 'Public again' })])
    act(() => result.current.state.retry())
    await waitFor(() => expect(result.current.results[0]?.title).toBe('Public again'))
})
it('exposes an inline request error and can retry into an honest empty result', async () => {
    vi.mocked(reads.readSearch).mockRejectedValue(new ApiError(400, 'Bad query'))
    const { result } = renderHook(() => usePlaceBrowse('en', null, 'place'), { wrapper: wrapper() })
    await waitFor(() => expect(result.current.state.error).toContain('Could not load'))
    vi.mocked(reads.readSearch).mockResolvedValue([])
    act(() => result.current.state.retry())
    await waitFor(() => expect(result.current.state.error).toBeNull())
    expect(result.current.state.pending).toBe(false)
    expect(result.current.results).toEqual([])
})
it('reports invalid links without fetching and updates nearby reference after a fresh location fix', async () => {
    const get = vi.fn<Geolocation['getCurrentPosition']>()
    vi.stubGlobal('navigator', { geolocation: { getCurrentPosition: get } })
    const { result } = renderHook(() => usePlaceBrowse('en', 'bad'), { wrapper: wrapper() })
    expect(result.current.detailState.error).toContain('invalid')
    expect(reads.readPublicPlace).not.toHaveBeenCalled()
    act(() => result.current.onView(mapView(31, 32, 121, 122, { lat: 31.5, lng: 121.5 }, 10)))
    act(() => result.current.chooseCategory('baby_room'))
    await waitFor(() =>
        expect(reads.readNearby).toHaveBeenCalledWith(
            expect.objectContaining({ lat: 31.5, lng: 121.5 }),
            'en',
            expect.any(AbortSignal),
        ),
    )
    expect(result.current.nearby?.located).toBe(false)
    act(() => result.current.location.locate())
    act(() =>
        get.mock.calls.at(-1)?.[0]({
            coords: { latitude: 32.12345678, longitude: 122 },
        } as GeolocationPosition),
    )
    await waitFor(() =>
        expect(reads.readNearby).toHaveBeenLastCalledWith(
            expect.objectContaining({ lat: 32.123457, lng: 122 }),
            'en',
            expect.any(AbortSignal),
        ),
    )
    expect(result.current.nearby?.located).toBe(true)
})

it('keeps a nearby search anchored while panning and cancels a superseded category request', async () => {
    let finishOld!: (value: ReturnType<typeof syntheticPlace>[]) => void
    vi.mocked(reads.readNearby).mockImplementation(({ category }) =>
        category === 'baby_room'
            ? new Promise((resolve) => {
                  finishOld = resolve
              })
            : Promise.resolve([syntheticPlace({ category, title: 'Newest nearby' })]),
    )
    const { result } = renderHook(() => usePlaceBrowse('en', null), { wrapper: wrapper() })
    act(() => result.current.onView(mapView(31, 32, 121, 122, { lat: 31.5, lng: 121.5 }, 10)))
    act(() => result.current.chooseCategory('baby_room'))
    await waitFor(() => expect(reads.readNearby).toHaveBeenCalledTimes(1))
    const signal = vi.mocked(reads.readNearby).mock.calls[0]![2]
    act(() => result.current.onView(mapView(32, 33, 122, 123, { lat: 32.5, lng: 122.5 }, 10)))
    expect(result.current.nearby?.point).toEqual({ lat: 31.5, lng: 121.5 })
    act(() => result.current.chooseCategory('friendly_clinic'))
    await waitFor(() => expect(result.current.results[0]?.title).toBe('Newest nearby'))
    expect(signal.aborted).toBe(true)
    await act(async () => finishOld([syntheticPlace({ title: 'Old nearby' })]))
    expect(result.current.results[0]?.title).toBe('Newest nearby')
})

it('reuses a buffered region for small pans and one zoom step, then refreshes across its boundary', async () => {
    const { result } = renderHook(() => usePlaceBrowse('en', null), { wrapper: wrapper() })
    const view = (south: number, west: number, size = 1, zoom = 14) =>
        mapView(
            south,
            south + size,
            west,
            west + size,
            { lat: south + size / 2, lng: west + size / 2 },
            zoom,
        )
    act(() => result.current.onView(view(31, 121)))
    await waitFor(() => expect(result.current.results).toHaveLength(1))
    expect(reads.readViewport).toHaveBeenCalledTimes(1)
    act(() => result.current.onView(view(31.2, 121.2)))
    await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 300))
    })
    expect(reads.readViewport).toHaveBeenCalledTimes(1)
    act(() => result.current.onView(view(31.25, 121.25, 0.5, 15)))
    await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 300))
    })
    expect(reads.readViewport).toHaveBeenCalledTimes(1)
    act(() => result.current.onView(view(31.6, 121.6)))
    await waitFor(() => expect(reads.readViewport).toHaveBeenCalledTimes(2))
    act(() => result.current.onView(view(31.75, 121.75, 0.25, 16)))
    await waitFor(() => expect(reads.readViewport).toHaveBeenCalledTimes(3))
})
it('retains loaded pins while fetching a new region and ignores superseded responses', async () => {
    let finishOld!: (value: ReturnType<typeof syntheticPlace>[]) => void
    const { result } = renderHook(() => usePlaceBrowse('en', null), { wrapper: wrapper() })
    act(() => result.current.onView(mapView(31, 32, 121, 122, { lat: 31.5, lng: 121.5 }, 14)))
    await waitFor(() => expect(result.current.results).toHaveLength(1))
    const original = result.current.markers[0]
    vi.mocked(reads.readViewport).mockImplementationOnce(
        () =>
            new Promise((resolve) => {
                finishOld = resolve
            }),
    )
    act(() => result.current.onView(mapView(33, 34, 123, 124, { lat: 33.5, lng: 123.5 }, 14)))
    await waitFor(() => expect(reads.readViewport).toHaveBeenCalledTimes(2))
    expect(result.current.markers[0]).toBe(original)
    const signal = vi.mocked(reads.readViewport).mock.calls[1]![2]
    vi.mocked(reads.readViewport).mockResolvedValue([
        syntheticPlace({ id: 2, title: 'New region' }),
    ])
    act(() => result.current.onView(mapView(35, 36, 125, 126, { lat: 35.5, lng: 125.5 }, 14)))
    await waitFor(() => expect(result.current.markers[0]?.id).toBe(2))
    expect(signal.aborted).toBe(true)
    await act(async () => finishOld([syntheticPlace({ title: 'Stale region' })]))
    expect(result.current.markers[0]?.title).toBe('New region')
})

// Acceptance regression from reported mobile zoom loss.
it('keeps previously loaded pins after a region request exhausts retries, then replaces them on recovery', async () => {
    const { result } = renderHook(() => usePlaceBrowse('en', null), { wrapper: wrapper() })
    act(() => result.current.onView(mapView(31, 32, 121, 122, { lat: 31.5, lng: 121.5 }, 14)))
    await waitFor(() => expect(result.current.markers).toHaveLength(1))
    const original = result.current.markers[0]!
    vi.mocked(reads.readViewport).mockRejectedValue(new ApiError(503, 'Offline'))
    act(() => result.current.onView(mapView(31, 32, 121, 122, { lat: 31.5, lng: 121.5 }, 16)))
    await waitFor(() => expect(result.current.state.error).toContain('temporarily unavailable'), {
        timeout: 3500,
    })
    expect(result.current.markers).toEqual([original])
    vi.mocked(reads.readViewport).mockResolvedValue([syntheticPlace({ id: 2, title: 'Recovered' })])
    act(() => result.current.state.retry())
    await waitFor(() => expect(result.current.markers[0]?.title).toBe('Recovered'))
})

it('keeps recovered pins across 20 zoom-band changes including cancelled and late responses', async () => {
    const { result } = renderHook(() => usePlaceBrowse('en', null), { wrapper: wrapper() })
    act(() => result.current.onView(mapView(31, 32, 121, 122, { lat: 31.5, lng: 121.5 }, 14)))
    await waitFor(() => expect(result.current.markers).toHaveLength(1))
    expect(result.current.markers[0]?.id).toBe(1)
    // Alternate an unrecoverable failure with successes and a hung (cancelled)
    // request so every band crossing exercises a different settle path.
    let n = 0
    vi.mocked(reads.readViewport).mockImplementation(() => {
        n += 1
        if (n % 5 === 2) return Promise.reject(new ApiError(503, 'Offline'))
        if (n % 5 === 4) return new Promise(() => {})
        return Promise.resolve([syntheticPlace({ id: 1 })])
    })
    for (let step = 0; step < 20; step += 1) {
        const zoom = 14 + (step % 6)
        const south = 31 + step * 0.001
        act(() =>
            result.current.onView(
                mapView(south, south + 1, 121, 122, { lat: south + 0.5, lng: 121.5 }, zoom),
            ),
        )
        await act(async () => {
            await new Promise((resolve) => setTimeout(resolve, 0))
        })
        expect(result.current.markers.map((m) => m.id)).toEqual([1])
    }
    vi.mocked(reads.readViewport).mockResolvedValue([syntheticPlace({ id: 9, title: 'Latest' })])
    // A far, previously unvisited center is outside every buffered window, so
    // it must issue a fresh read and replace the retained fallback.
    act(() => result.current.onView(mapView(48, 49, 138, 139, { lat: 48.5, lng: 138.5 }, 14)))
    await waitFor(() => expect(result.current.markers[0]?.title).toBe('Latest'), { timeout: 4000 })
})

it('clears the fallback when a successful region read is honestly empty', async () => {
    const { result } = renderHook(() => usePlaceBrowse('en', null), { wrapper: wrapper() })
    act(() => result.current.onView(mapView(31, 32, 121, 122, { lat: 31.5, lng: 121.5 }, 14)))
    await waitFor(() => expect(result.current.markers).toHaveLength(1))
    vi.mocked(reads.readViewport).mockResolvedValue([])
    act(() => result.current.onView(mapView(31, 32, 121, 122, { lat: 31.5, lng: 121.5 }, 16)))
    await waitFor(() => expect(result.current.markers).toEqual([]))
    // A later failure must not revive the cleared pins.
    vi.mocked(reads.readViewport).mockRejectedValue(new ApiError(503, 'Offline'))
    act(() => result.current.onView(mapView(31, 32, 121, 122, { lat: 31.5, lng: 121.5 }, 18)))
    await waitFor(() => expect(result.current.state.error).toContain('temporarily unavailable'), {
        timeout: 4000,
    })
    expect(result.current.markers).toEqual([])
})

it('never falls back to another language after a region failure', async () => {
    vi.mocked(reads.readViewport).mockResolvedValue([
        syntheticPlace({ id: 1, title: 'English pin' }),
    ])
    const { result, rerender } = renderHook(
        ({ lang }: { lang: 'en' | 'zh' }) => usePlaceBrowse(lang, null),
        { initialProps: { lang: 'en' as 'en' | 'zh' }, wrapper: wrapper() },
    )
    act(() => result.current.onView(mapView(31, 32, 121, 122, { lat: 31.5, lng: 121.5 }, 14)))
    await waitFor(() => expect(result.current.markers).toHaveLength(1))
    vi.mocked(reads.readViewport).mockRejectedValue(new ApiError(503, 'Offline'))
    rerender({ lang: 'zh' })
    act(() => result.current.onView(mapView(31, 32, 121, 122, { lat: 31.5, lng: 121.5 }, 14)))
    await waitFor(() => expect(result.current.state.error).toContain('temporarily unavailable'), {
        timeout: 4000,
    })
    expect(result.current.markers).toEqual([])
})

it('keeps a 404-pruned point dead after closing detail while the region keeps failing, then restores on success', async () => {
    const { result, rerender } = renderHook(
        ({ id }: { id: string | null }) => usePlaceBrowse('en', id),
        { initialProps: { id: '1' as string | null }, wrapper: wrapper() },
    )
    act(() => result.current.onView(mapView(31, 32, 121, 122, { lat: 31.5, lng: 121.5 }, 14)))
    await waitFor(() => expect(result.current.markers).toHaveLength(1))
    await waitFor(() => expect(result.current.detail?.id).toBe(1))
    // A new region that ultimately fails: the last success is retained.
    vi.mocked(reads.readViewport).mockRejectedValue(new ApiError(503, 'Offline'))
    act(() => result.current.onView(mapView(31, 32, 121, 122, { lat: 31.5, lng: 121.5 }, 16)))
    await waitFor(() => expect(result.current.state.error).toContain('temporarily unavailable'), {
        timeout: 4000,
    })
    expect(result.current.markers).toHaveLength(1)
    // Detail confirms a 404; the retained fallback must be pruned, not merely
    // filtered while `unavailable` happens to be true.
    vi.mocked(reads.readPublicPlace).mockRejectedValue(new ApiError(404, 'HTTP 404'))
    act(() => result.current.detailState.retry())
    await waitFor(() => expect(result.current.detailState.error).toContain('unavailable'))
    // Close the detail: `unavailable` is no longer true, so a surviving fallback
    // would resurrect the withdrawn marker.
    rerender({ id: null })
    act(() => result.current.onView(mapView(31, 32, 121, 122, { lat: 31.5, lng: 121.5 }, 17)))
    await waitFor(() => expect(result.current.state.error).toContain('temporarily unavailable'), {
        timeout: 4000,
    })
    expect(result.current.markers).toEqual([])
    // A later successful public read may honestly restore it.
    vi.mocked(reads.readViewport).mockResolvedValue([syntheticPlace({ id: 1, title: 'Back' })])
    act(() => result.current.onView(mapView(41, 42, 131, 132, { lat: 41.5, lng: 131.5 }, 14)))
    await waitFor(() => expect(result.current.markers[0]?.title).toBe('Back'), { timeout: 4000 })
})
