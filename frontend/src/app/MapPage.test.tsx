import { PreferencesProvider } from '@/features/preferences/PreferencesProvider'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter, useLocation } from 'react-router'
import { afterEach, expect, it, vi } from 'vitest'
import { LanguageProvider } from '@/shared/i18n'
import { syntheticPlace, syntheticPlaces } from '@/features/dev/placeFixtures'
import { MapPage } from './MapPage'
const clients: QueryClient[] = []
afterEach(() => {
    cleanup()
    localStorage.clear()
    clients.splice(0).forEach((client) => client.clear())
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
    vi.unstubAllEnvs()
})
function Route() {
    const route = useLocation()
    return (
        <output data-testid="route">
            {route.pathname}
            {route.search}
            {route.hash}
        </output>
    )
}
function PageProviders() {
    const lang = new URLSearchParams(useLocation().search).get('lang')
    return (
        <LanguageProvider override={lang === 'en' || lang === 'zh' ? lang : undefined}>
            <PreferencesProvider>
                <MapPage />
                <Route />
            </PreferencesProvider>
        </LanguageProvider>
    )
}
function app(url: string, mobile = false, count = 5) {
    vi.stubGlobal('innerWidth', mobile ? 375 : 1440)
    vi.stubGlobal('matchMedia', () => ({
        matches: mobile,
        addEventListener: () => {},
        removeEventListener: () => {},
    }))
    vi.spyOn(HTMLElement.prototype, 'getClientRects').mockImplementation(
        () => [{ width: 100, height: 40 }] as unknown as DOMRectList,
    )
    const fetcher = vi.fn<typeof fetch>(async (input) => {
        const url = new URL(String(input), 'http://localhost')
        const id = /\/api\/markers\/(\d+)$/.exec(url.pathname)?.[1]
        const data = id
            ? syntheticPlace({
                  id: Number(id),
                  title: `Synthetic place ${id}`,
                  markImage: '/uploads/markers/synthetic.webp',
              })
            : syntheticPlaces(count).map((place) =>
                  url.pathname.endsWith('/nearby')
                      ? {
                            ...place,
                            markImage: '/uploads/markers/synthetic.webp',
                            category: url.searchParams.get('category'),
                        }
                      : place,
              )
        return new Response(JSON.stringify(data), {
            headers: { 'Content-Type': 'application/json' },
        })
    })
    vi.stubGlobal('fetch', fetcher)
    const client = new QueryClient({
        defaultOptions: { queries: { retry: false, gcTime: Infinity } },
    })
    clients.push(client)
    const view = render(
        <QueryClientProvider client={client}>
            <MemoryRouter initialEntries={[url]}>
                <PageProviders />
            </MemoryRouter>
        </QueryClientProvider>,
    )
    return { ...view, fetcher }
}
it('opens a result and dismisses the desktop second column while retaining the real map', async () => {
    const { container } = app('/search?q=place&lang=en#retained')
    const map = container.querySelector('.leaflet-container')
    const row = await screen.findByRole('button', { name: /^Synthetic place 1\s*08:00/ })
    fireEvent.click(row)
    await screen.findByRole('heading', { name: 'Synthetic place 1' })
    expect(screen.getByTestId('route')).toHaveTextContent('markerId=1#retained')
    fireEvent.click(screen.getByRole('button', { name: 'Close panel' }))
    await waitFor(() =>
        expect(screen.queryByRole('heading', { name: 'Details' })).not.toBeInTheDocument(),
    )
    expect(screen.queryByRole('heading', { name: 'Search Results' })).not.toBeInTheDocument()
    expect(container.querySelector('.leaflet-container')).toBe(map)
})
it.each([
    ['/?lang=en', 'half'],
    ['/?lang=en&snap=collapsed', 'collapsed'],
])('preserves phone input and IME composition when expanding %s from %s', async (url, snap) => {
    app(url, true)
    expect(document.getElementById('map-shell')).toHaveAttribute('data-snap', snap)
    const input = screen.getByRole('textbox', { name: 'Search Positions' })
    act(() => input.focus())
    fireEvent.compositionStart(input)
    fireEvent.change(input, { target: { value: '医' } })
    await waitFor(() =>
        expect(document.getElementById('map-shell')).toHaveAttribute('data-snap', 'full'),
    )
    expect(screen.getByRole('textbox', { name: 'Search Positions' })).toBe(input)
    expect(input).toHaveFocus()
    fireEvent.change(input, { target: { value: '医院' } })
    fireEvent.compositionEnd(input)
    expect(input).toHaveValue('医院')
    expect(input).toHaveFocus()
    expect(document.getElementById('map-shell')).toHaveAttribute('data-panel', 'search')
})
it('opens the Nearby list from the phone radar and dismisses a direct search with Escape', async () => {
    app('/maps?markerId=1&lang=en', true)
    await screen.findByRole('heading', { name: 'Synthetic place 1' })
    fireEvent.click(screen.getByRole('button', { name: 'Find nearby' }))
    await screen.findByRole('list', { name: 'Accessible Toilets in 1km' })
    expect(document.getElementById('map-shell')).toHaveAttribute('data-panel', 'nearby')
    expect(document.getElementById('map-shell')).toHaveAttribute('data-snap', 'full')
    expect(screen.getByTestId('route')).not.toHaveTextContent('markerId')
    cleanup()
    localStorage.clear()
    app('/search?q=place&lang=en', true)
    await screen.findByRole('list', { name: 'Search Results' })
    fireEvent.keyDown(screen.getByRole('textbox', { name: 'Search Positions' }), { key: 'Escape' })
    await waitFor(() =>
        expect(document.getElementById('map-shell')).toHaveAttribute('data-snap', 'collapsed'),
    )
})
it('does not activate ignored coordinates when dismissing a mixed legacy marker link', async () => {
    const { container } = app('/maps?markerId=1&lat=0&lng=0&title=Other&lang=en')
    await screen.findByRole('heading', { name: 'Synthetic place 1' })
    const before = (container.querySelector('.product-map') as HTMLElement).dataset.center
    fireEvent.click(screen.getByRole('button', { name: 'Close panel' }))
    await waitFor(() =>
        expect(screen.queryByRole('heading', { name: 'Details' })).not.toBeInTheDocument(),
    )
    expect(container.querySelector('#map-shared-location')).toBeNull()
    expect((container.querySelector('.product-map') as HTMLElement).dataset.center).toBe(before)
})
it('shows legacy phone search results immediately, and restores a virtualized far row after closing detail', async () => {
    app('/search?q=place&lang=en', true, 300)
    const list = await screen.findByRole('list', { name: 'Search Results' })
    expect(document.getElementById('map-shell')).toHaveAttribute('data-snap', 'full')
    expect(screen.getAllByRole('listitem').length).toBeLessThan(30)
    fireEvent.keyDown(list, { key: 'End' })
    const last = await screen.findByRole('button', { name: /^Synthetic place 300\s*08:00/ })
    await waitFor(() => expect(last).toHaveFocus())
    const offset = list.scrollTop
    fireEvent.click(last)
    await screen.findByRole('heading', { name: 'Synthetic place 300' })
    fireEvent.click(screen.getByRole('button', { name: 'Close panel' }))
    const restored = await screen.findByRole('button', { name: /^Synthetic place 300\s*08:00/ })
    await waitFor(() => expect(restored).toHaveFocus())
    expect(screen.getByRole('list', { name: 'Search Results' }).scrollTop).toBe(offset)
    expect(screen.getAllByRole('listitem').length).toBeLessThan(30)
})
it.each(['/maps?markerId=unsafe', '/maps?panel=details'])(
    'handles invalid details %s without an empty panel',
    async (url) => {
        const { fetcher } = app(url)
        expect((await screen.findAllByText('This place link is invalid.')).length).toBeGreaterThan(
            0,
        )
        expect(
            fetcher.mock.calls.some(
                ([path]) =>
                    /\/api\/markers\/[^/?]+\?/.test(String(path)) &&
                    !String(path).includes('viewport'),
            ),
        ).toBe(false)
    },
)
it('renders a legacy shared coordinate target and gives markerId precedence over coordinates', async () => {
    const { container } = app('/maps?lat=31.2&lng=121.4&title=Meeting%20Point')
    expect(container.querySelector('#map-shared-location')).toHaveAttribute(
        'title',
        'Meeting Point',
    )
    cleanup()
    localStorage.clear()
    const second = app('/maps?markerId=1&lat=31.2&lng=121.4&title=Meeting%20Point&lang=zh')
    await screen.findByRole('heading', { name: 'Synthetic place 1' })
    expect(second.container.querySelector('#map-shared-location')).toBeNull()
    expect(second.fetcher).toHaveBeenCalledWith(
        '/api/markers/1?lang=zh',
        expect.objectContaining({ credentials: 'omit' }),
    )
})
it('copies only a public place link, offers navigation apps without auto-opening one, and removes broken photos', async () => {
    app('/maps?markerId=1&lang=en&lat=0&lng=0')
    const writeText = vi.fn(async () => {})
    vi.stubGlobal('navigator', { clipboard: { writeText } })
    await screen.findByRole('heading', { name: 'Synthetic place 1' })
    const photo = document.querySelector<HTMLImageElement>('.place-photo img')!
    expect(photo).toHaveAttribute('src', '/uploads/markers/synthetic.webp?variant=detail')
    fireEvent.error(photo)
    fireEvent.error(document.querySelector<HTMLImageElement>('.place-photo img')!)
    expect(document.querySelector('.place-photo')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Share' }))
    expect(await screen.findByText('Link copied.')).toBeInTheDocument()
    expect(writeText).toHaveBeenCalledWith(`${window.location.origin}/maps?markerId=1&lang=en`)
    // The destination is not opened until the user explicitly chooses an app.
    expect(screen.queryByRole('dialog', { name: 'Choose a navigation app' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Navigate' }))
    const navigateTrigger = screen.getByRole('button', { name: 'Navigate' })
    const chooser = screen.getByRole('dialog', { name: 'Choose a navigation app' })
    const links = within(chooser).getAllByRole('link')
    expect(links.map((link) => link.textContent)).toEqual([
        'Apple Maps',
        'Google Maps',
        'AMap',
        'Tencent Maps',
    ])
    expect(links[0]).toHaveAttribute(
        'href',
        'https://maps.apple.com/?daddr=31.2304%2C121.4737&dirflg=w',
    )
    expect(links[1]).toHaveAttribute(
        'href',
        'https://www.google.com/maps/dir/?api=1&travelmode=walking&destination=31.2304%2C121.4737',
    )
    expect(links[2]).toHaveAttribute(
        'href',
        'https://uri.amap.com/marker?position=121.4737%2C31.2304&name=Synthetic+place+1&coordinate=wgs84&src=lycoris-map&callnative=1',
    )
    expect(links[3]).toHaveAttribute(
        'href',
        'https://apis.map.qq.com/uri/v1/routeplan?type=walk&to=Synthetic+place+1&tocoord=31.2304%2C121.4737&coord_type=1&referer=Lycoris+Maps',
    )
    for (const link of links) expect(link).toHaveAttribute('rel', 'noopener noreferrer')
    // Escape closes the menu and returns focus to the trigger.
    fireEvent.keyDown(chooser, { key: 'Escape' })
    await waitFor(() =>
        expect(screen.queryByRole('dialog', { name: 'Choose a navigation app' })).toBeNull(),
    )
    expect(navigateTrigger).toHaveAttribute('aria-expanded', 'false')
})

const nearbyCategories = [
    ['Accessible Toilets', 'accessible_toilet', 'toilet'],
    ['Nursing Rooms', 'baby_room', 'nursing'],
    ['Medical Institutions', 'friendly_clinic', 'medical'],
] as const
it.each(
    nearbyCategories.flatMap(([label, category, card]) =>
        [false, true].map((mobile) => ({ label, category, card, mobile })),
    ),
)(
    'opens the $category Nearby list from a category card (mobile=$mobile)',
    async ({ label, category, card, mobile }) => {
        const { container, fetcher } = app('/?panel=search&snap=full&lang=en#retained', mobile)
        const map = container.querySelector('.leaflet-container')
        fireEvent.click(screen.getByRole('button', { name: label }))
        const list = await screen.findByRole('list', { name: `${label} in 1km` })
        expect(screen.getByRole('heading', { name: 'Nearby' })).toBeInTheDocument()
        expect(screen.queryByRole('textbox')).not.toBeInTheDocument()
        expect(
            fetcher.mock.calls.some(([input]) => {
                const url = new URL(String(input), 'http://localhost')
                return (
                    url.pathname === '/api/markers/nearby' &&
                    url.searchParams.get('category') === category &&
                    url.searchParams.get('radius') === '1000'
                )
            }),
        ).toBe(true)
        expect(within(list).getAllByRole('button', { name: 'Share' })).toHaveLength(5)
        expect(within(list).getAllByRole('button', { name: 'Navigate' })).toHaveLength(5)
        expect(screen.getByTestId('route')).toHaveTextContent(`nearbyCategory=${category}#retained`)
        fireEvent.click(
            screen.getByRole('button', { name: mobile ? 'Close nearby' : 'Close panel' }),
        )
        await waitFor(() =>
            expect(screen.queryByRole('heading', { name: 'Nearby' })).not.toBeInTheDocument(),
        )
        expect(container.querySelector('.leaflet-container')).toBe(map)
        if (mobile) {
            expect(screen.getByRole('heading', { name: 'Find Nearby' })).toBeInTheDocument()
            expect(screen.getByRole('button', { name: label })).toHaveFocus()
            expect(document.getElementById(`mobile-nearby-${card}`)).toHaveFocus()
        } else {
            expect(container.querySelector('.desktop-panel')).toBeNull()
            expect(screen.getByRole('button', { name: 'Search' })).toHaveFocus()
        }
    },
)
it('restores the phone Nearby category, scroll and focused title after viewing a place', async () => {
    app('/?panel=search&snap=full&lang=en', true)
    fireEvent.click(screen.getByRole('button', { name: 'Nursing Rooms' }))
    const list = await screen.findByRole('list', { name: 'Nursing Rooms in 1km' })
    fireEvent.scroll(list, { target: { scrollTop: 400 } })
    fireEvent.click(within(list).getByRole('button', { name: 'Synthetic place 3' }))
    await screen.findByRole('heading', { name: 'Synthetic place 3' })
    fireEvent.click(screen.getByRole('button', { name: 'Close panel' }))
    const restored = await screen.findByRole('list', { name: 'Nursing Rooms in 1km' })
    expect(restored.scrollTop).toBe(400)
    expect(within(restored).getByRole('button', { name: 'Synthetic place 3' })).toHaveFocus()
    expect(document.getElementById('map-shell')).toHaveAttribute('data-snap', 'full')
})
it('initializes a direct Nearby URL after map readiness, renders empty results, and closes within the app', async () => {
    const { fetcher } = app('/?panel=nearby&nearbyCategory=friendly_clinic&lang=en#kept', true, 0)
    await screen.findByText('No places found.')
    expect(screen.getByRole('heading', { name: 'Medical Institutions in 1km' })).toBeInTheDocument()
    expect(
        fetcher.mock.calls.some(([input]) => String(input).includes('category=friendly_clinic')),
    ).toBe(true)
    fireEvent.click(screen.getByRole('button', { name: 'Close nearby' }))
    expect(document.getElementById('map-shell')).toHaveAttribute('data-snap', 'collapsed')
    expect(screen.getByTestId('route')).toHaveTextContent('#kept')
})

it('windows a long Nearby list and restores a far item after mobile detail without losing its place', async () => {
    app('/?panel=nearby&nearbyCategory=baby_room&lang=en', true, 500)
    const list = await screen.findByRole('list', { name: 'Nursing Rooms in 1km' })
    expect(screen.getAllByRole('listitem').length).toBeLessThan(25)
    fireEvent.keyDown(list, { key: 'End' })
    const last = await within(list).findByRole('button', { name: 'Synthetic place 500' })
    await waitFor(() => expect(last).toHaveFocus())
    const offset = list.scrollTop
    fireEvent.click(last)
    await screen.findByRole('heading', { name: 'Synthetic place 500' })
    fireEvent.click(screen.getByRole('button', { name: 'Close panel' }))
    await waitFor(() =>
        expect(
            within(screen.getByRole('list', { name: 'Nursing Rooms in 1km' })).getByRole('button', {
                name: 'Synthetic place 500',
            }),
        ).toHaveFocus(),
    )
    expect(screen.getByRole('list', { name: 'Nursing Rooms in 1km' }).scrollTop).toBe(offset)
    expect(screen.getAllByRole('listitem').length).toBeLessThan(25)
})
it('shares the selected Nearby item, uses its destination and hides a failed image', async () => {
    app('/?panel=nearby&lang=en')
    const writeText = vi.fn(async () => {})
    vi.stubGlobal('navigator', { clipboard: { writeText } })
    const item = await screen.findByRole('article', { name: 'Synthetic place 2' })
    fireEvent.click(within(item).getByRole('button', { name: 'Share' }))
    expect(await within(item).findByText('Link copied.')).toBeInTheDocument()
    expect(writeText).toHaveBeenCalledWith(`${window.location.origin}/maps?markerId=2&lang=en`)
    const photo = item.querySelector('.place-photo img')!
    expect(photo).toHaveAttribute('src', '/uploads/markers/synthetic.webp?variant=thumb')
    fireEvent.error(photo)
    fireEvent.error(item.querySelector<HTMLImageElement>('.place-photo img')!)
    expect(item.querySelector('.place-photo')).toBeNull()
    const navigate = within(item).getByRole('button', { name: 'Navigate' })
    fireEvent.click(navigate)
    const chooser = screen.getByRole('dialog', { name: 'Choose a navigation app' })
    expect(within(chooser).getAllByRole('link')).toHaveLength(4)
    for (const link of within(chooser).getAllByRole('link'))
        expect(link).toHaveAttribute('rel', 'noopener noreferrer')
    fireEvent.keyDown(chooser, { key: 'Escape' })
    await waitFor(() =>
        expect(screen.queryByRole('dialog', { name: 'Choose a navigation app' })).toBeNull(),
    )
    expect(navigate).toHaveFocus()
})

it('keeps End and the return target visible when measured Nearby cards exceed their estimated height', async () => {
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (
        this: HTMLElement,
    ) {
        return { width: 288, height: this.classList.contains('nearby-row') ? 1000 : 0 } as DOMRect
    })
    const observers = new Set<ResizeObserverCallback>()
    vi.stubGlobal(
        'ResizeObserver',
        class {
            callback: ResizeObserverCallback
            constructor(callback: ResizeObserverCallback) {
                this.callback = callback
            }
            observe() {
                observers.add(this.callback)
            }
            unobserve() {}
            disconnect() {
                observers.delete(this.callback)
            }
        },
    )
    app('/?panel=nearby&lang=en', true, 500)
    const list = await screen.findByRole('list', { name: 'Accessible Toilets in 1km' })
    fireEvent.keyDown(list, { key: 'End' })
    await act(async () => {
        for (const callback of [...observers]) callback([], {} as ResizeObserver)
    })
    const last = await within(list).findByRole('button', { name: 'Synthetic place 500' })
    await waitFor(() => expect(last).toHaveFocus())
    expect(screen.getAllByRole('listitem').length).toBeLessThan(25)
    const offset = list.scrollTop
    fireEvent.click(last)
    await screen.findByRole('heading', { name: 'Synthetic place 500' })
    fireEvent.click(screen.getByRole('button', { name: 'Close panel' }))
    const restored = await screen.findByRole('list', { name: 'Accessible Toilets in 1km' })
    await waitFor(() =>
        expect(within(restored).getByRole('button', { name: 'Synthetic place 500' })).toHaveFocus(),
    )
    expect(restored.scrollTop).toBe(offset)
})

it('keeps a chosen phone language after closing the panel and updates the real requests without replacing the map', async () => {
    const { container, fetcher } = app('/?lang=en&snap=full', true)
    const map = container.querySelector('.leaflet-container')
    fireEvent.click(screen.getByRole('button', { name: 'Choose Language English' }))
    fireEvent.click(screen.getByRole('radio', { name: '简体中文' }))
    expect(document.documentElement.lang).toBe('zh-CN')
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
    await screen.findByRole('button', { name: '选择语言 简体中文' })
    expect(container.querySelector('.leaflet-container')).toBe(map)
    await waitFor(() =>
        expect(fetcher.mock.calls.some(([url]) => String(url).includes('lang=zh'))).toBe(true),
    )
    expect(localStorage.getItem('lycoris.language')).toBe('zh')
})
it('uses a saved range and radar category while the category cards remain explicit', async () => {
    localStorage.setItem(
        'lycoris.map-preferences',
        JSON.stringify({ radius: 2500, category: 'baby_room', source: 'osm' }),
    )
    const { fetcher } = app('/?lang=en', true)
    fireEvent.click(screen.getByRole('button', { name: 'Find nearby' }))
    await screen.findByRole('heading', { name: 'Nursing Rooms in 2.5km' })
    await waitFor(() =>
        expect(
            fetcher.mock.calls.some(
                ([url]) =>
                    String(url).includes('radius=2500') &&
                    String(url).includes('category=baby_room'),
            ),
        ).toBe(true),
    )
})

it.each([
    ['Choose Language English', 'Languages'],
    ['Searching Range 1km', 'Range'],
    ['Searching Type Toilet', 'Category'],
    ['Map Source OSM', 'Map Source'],
    ['About Lycoris Maps', 'About'],
])('closes the phone %s option and restores the main menu', async (option, title) => {
    const { container } = app('/?lang=en&snap=full', true)
    const map = container.querySelector('.leaflet-container')
    fireEvent.click(screen.getByRole('button', { name: option }))
    const popup = screen.getByRole('dialog', { name: title })
    expect(popup).toBeInTheDocument()
    expect(screen.getByTestId('route')).toHaveTextContent('?lang=en&snap=full')
    fireEvent.click(screen.getByRole('button', { name: option }))
    await waitFor(() =>
        expect(screen.queryByRole('dialog', { name: title })).not.toBeInTheDocument(),
    )
    expect(screen.getByRole('button', { name: option })).toHaveFocus()
    expect(document.getElementById('map-shell')).toHaveAttribute('data-snap', 'full')
    expect(container.querySelector('.leaflet-container')).toBe(map)
})
it.each([false, true])(
    'switches both map layers without changing the camera, pins or route (mobile=%s)',
    async (mobile) => {
        vi.stubEnv('VITE_TIANDITU_API_KEY', 'browser-test-key')
        const url = '/?lang=en&snap=collapsed'
        const { container } = app(url, mobile)
        const map = container.querySelector<HTMLElement>('.leaflet-container')!
        const camera = { ...map.dataset }
        await waitFor(() => expect(map.querySelector('.leaflet-marker-icon')).toBeInTheDocument())
        const marker = map.querySelector('.leaflet-marker-icon')
        const trigger = screen.getByRole('button', { name: 'Map source' })
        fireEvent.click(trigger)
        const popup = screen.getByRole('dialog', { name: 'Map Source' })
        const osm = within(popup).getByRole('radio', { name: 'OSM' })
        expect(osm).toBeChecked()
        expect(osm).toHaveFocus()
        expect(within(popup).getAllByRole('radio')).toHaveLength(3)
        expect(within(popup).queryByRole('radio', { name: 'Google Maps' })).not.toBeInTheDocument()
        fireEvent.click(within(popup).getByRole('radio', { name: '天地图' }))
        await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
        expect(trigger).toHaveFocus()
        expect(screen.getByTestId('route')).toHaveTextContent(url)
        expect(container.querySelector('.leaflet-container')).toBe(map)
        expect({ ...map.dataset }).toEqual(camera)
        expect(map.querySelector('.leaflet-marker-icon')).toBe(marker)
        expect(map.querySelector('img[src*="/vec_w/wmts?"]')).toBeInTheDocument()
        expect(map.querySelector('img[src*="/cva_w/wmts?"]')).toBeInTheDocument()
        expect(map.querySelector('img[src*="tile.openstreetmap.org"]')).not.toBeInTheDocument()
        expect(within(map).getByRole('link', { name: '天地图' })).toBeInTheDocument()
        expect(JSON.parse(localStorage.getItem('lycoris.map-preferences')!)).toMatchObject({
            source: 'tianditu',
        })
        fireEvent.click(trigger)
        const selected = screen.getByRole('radio', { name: '天地图' })
        expect(selected).toBeChecked()
        expect(selected).toHaveFocus()
        fireEvent.keyDown(selected, { key: 'Escape' })
        await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
        expect(trigger).toHaveFocus()
        fireEvent.click(trigger)
        fireEvent.click(screen.getByRole('radio', { name: 'OSM' }))
        await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
        expect(map.querySelector('img[src^="/tiles/osm/"]')).toBeInTheDocument()
        expect(map.querySelector('img[src*="tianditu.gov.cn"]')).not.toBeInTheDocument()
        expect(within(map).queryByRole('link', { name: '天地图' })).not.toBeInTheDocument()
        expect({ ...map.dataset }).toEqual(camera)
    },
)
it('keeps unconfigured Tianditu unavailable and uses OSM for a saved Tianditu preference', () => {
    vi.stubEnv('VITE_TIANDITU_API_KEY', '')
    localStorage.setItem('lycoris.map-preferences', '{"source":"tianditu"}')
    const { container } = app('/?lang=en')
    fireEvent.click(screen.getByRole('button', { name: 'Map source' }))
    const unavailable = screen.getByRole('radio', { name: '天地图' })
    expect(unavailable).toBeDisabled()
    expect(unavailable).toHaveAccessibleDescription('Not available yet')
    fireEvent.click(unavailable)
    expect(screen.getByRole('radio', { name: 'OSM' })).toBeChecked()
    expect(container.querySelector('img[src*="tianditu.gov.cn"]')).not.toBeInTheDocument()
})
it('restores Tianditu on reload and reflects source changes in Settings', async () => {
    vi.stubEnv('VITE_TIANDITU_API_KEY', 'browser-test-key')
    localStorage.setItem('lycoris.map-preferences', '{"source":"tianditu"}')
    const { container } = app('/?lang=en&panel=settings')
    expect(container.querySelector('img[src*="/vec_w/wmts?"]')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Map Source 天地图' }))
    expect(screen.getByRole('radio', { name: '天地图' })).toBeChecked()
    fireEvent.click(screen.getByRole('radio', { name: 'OSM' }))
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
    expect(screen.getByRole('button', { name: 'Map Source OSM' })).toBeInTheDocument()
    expect(container.querySelector('img[src*="tianditu.gov.cn"]')).not.toBeInTheDocument()
})
it('closes phone Nearby back to the default half menu without remounting the map', async () => {
    const { container } = app('/?lang=en', true)
    const map = container.querySelector('.leaflet-container')
    fireEvent.click(screen.getByRole('button', { name: 'Find nearby' }))
    await screen.findByRole('heading', { name: 'Nearby' })
    fireEvent.click(screen.getByRole('button', { name: 'Close panel' }))
    await waitFor(() =>
        expect(screen.queryByRole('heading', { name: 'Nearby' })).not.toBeInTheDocument(),
    )
    expect(document.getElementById('map-shell')).toHaveAttribute('data-snap', 'half')
    expect(screen.getByRole('button', { name: 'Find nearby' })).toHaveFocus()
    expect(container.querySelector('.leaflet-container')).toBe(map)
})

function pullSecondarySheetDown() {
    const handle = document.getElementById('sheet-handle')!
    const section = handle.closest('section')!
    vi.spyOn(section, 'getBoundingClientRect').mockReturnValue({
        x: 0,
        y: 46,
        top: 46,
        left: 0,
        right: 375,
        bottom: 667,
        width: 375,
        height: 621,
        toJSON: () => ({}),
    })
    vi.spyOn(section.parentElement!, 'getBoundingClientRect').mockReturnValue({
        x: 0,
        y: 0,
        top: 0,
        left: 0,
        right: 375,
        bottom: 667,
        width: 375,
        height: 667,
        toJSON: () => ({}),
    })
    for (const [type, y] of [
        ['start', 55],
        ['move', 660],
        ['end', 660],
    ] as const) {
        const point = { identifier: 1, clientX: 180, clientY: y }
        fireEvent(
            handle,
            Object.assign(new Event(`touch${type}`, { bubbles: true, cancelable: true }), {
                touches: type === 'end' ? [] : [point],
                changedTouches: [point],
            }),
        )
    }
}

it.each([
    ['radar', 'collapsed', 'Find nearby'],
    ['card', 'half', 'Accessible Toilets'],
    ['card', 'full', 'Accessible Toilets'],
])(
    'returns Nearby opened through the %s to the %s menu exactly like X',
    async (_entry, snap, opener) => {
        const { container } = app(`/?lang=en&snap=${snap}#retained`, true)
        const map = container.querySelector('.leaflet-container')
        fireEvent.click(screen.getByRole('button', { name: opener }))
        await screen.findByRole('list', { name: 'Accessible Toilets in 1km' })
        fireEvent.click(screen.getByRole('button', { name: 'Close panel' }))
        await waitFor(() =>
            expect(screen.queryByRole('heading', { name: 'Nearby' })).not.toBeInTheDocument(),
        )
        const expectedRoute = screen.getByTestId('route').textContent
        fireEvent.click(screen.getByRole('button', { name: opener }))
        await screen.findByRole('list', { name: 'Accessible Toilets in 1km' })
        pullSecondarySheetDown()
        await waitFor(() =>
            expect(screen.queryByRole('heading', { name: 'Nearby' })).not.toBeInTheDocument(),
        )
        expect(screen.getByTestId('route').textContent).toBe(expectedRoute)
        expect(document.getElementById('map-shell')).toHaveAttribute('data-snap', snap)
        expect(screen.getByRole('textbox', { name: 'Search Positions' })).toBeInTheDocument()
        expect(screen.getByRole('button', { name: opener })).toHaveFocus()
        expect(container.querySelector('.leaflet-container')).toBe(map)
        expect(
            document.getElementById('map-shell')!.style.getPropertyValue('--sheet-visual-height'),
        ).toBe('')
        expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    },
)

it('closes a directly opened Nearby by dragging, without leaving the app', async () => {
    const { container } = app('/?lang=en&panel=nearby&nearbyCategory=baby_room#retained', true)
    await screen.findByRole('list', { name: 'Nursing Rooms in 1km' })
    const map = container.querySelector('.leaflet-container')
    pullSecondarySheetDown()
    await waitFor(() =>
        expect(screen.queryByRole('heading', { name: 'Nearby' })).not.toBeInTheDocument(),
    )
    expect(document.getElementById('map-shell')).toHaveAttribute('data-panel', 'initial')
    expect(document.getElementById('map-shell')).toHaveAttribute('data-snap', 'collapsed')
    expect(screen.getByRole('textbox', { name: 'Search Positions' })).toBeInTheDocument()
    expect(screen.getByTestId('route')).toHaveTextContent('#retained')
    expect(container.querySelector('.leaflet-container')).toBe(map)
})

it('fits the open primary menu to changing content and caps it to the viewport', () => {
    vi.stubGlobal('innerHeight', 844)
    let contentHeight = 580
    let nearbyHeight = 144
    const bounds = HTMLElement.prototype.getBoundingClientRect
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (
        this: HTMLElement,
    ) {
        return this.classList.contains('mobile-search-content')
            ? new DOMRect(0, 0, 375, contentHeight)
            : this.classList.contains('nearby-cards')
              ? new DOMRect(11, 160, 353, nearbyHeight)
              : bounds.call(this)
    })
    const observed = new Map<Element, ResizeObserverCallback>()
    vi.stubGlobal(
        'ResizeObserver',
        class {
            callback: ResizeObserverCallback
            constructor(callback: ResizeObserverCallback) {
                this.callback = callback
            }
            observe(element: Element) {
                observed.set(element, this.callback)
            }
            unobserve(element: Element) {
                observed.delete(element)
            }
            disconnect() {
                for (const [element, callback] of observed)
                    if (callback === this.callback) observed.delete(element)
            }
        },
    )
    const { container } = app('/?lang=en', true)
    const root = document.getElementById('map-shell')!
    const content = container.querySelector('.mobile-search-content')!
    const nearby = container.querySelector('.nearby-cards')!
    expect(root).toHaveAttribute('data-snap', 'half')
    expect(root.style.getPropertyValue('--sheet-height')).toBe('326px')
    for (const name of ['Accessible Toilets', 'Nursing Rooms', 'Medical Institutions'])
        expect(screen.getByRole('button', { name })).toBeInTheDocument()
    fireEvent.keyDown(screen.getByRole('button', { name: 'Change panel height (half)' }), {
        key: 'ArrowUp',
    })
    expect(root.style.getPropertyValue('--sheet-height')).toBe('580px')
    contentHeight = 1200
    act(() => observed.get(content)!([], {} as ResizeObserver))
    expect(root.style.getPropertyValue('--sheet-height')).toBe('798px')
    contentHeight = 620
    act(() => observed.get(content)!([], {} as ResizeObserver))
    expect(root.style.getPropertyValue('--sheet-height')).toBe('620px')
    vi.stubGlobal('innerHeight', 600)
    fireEvent.resize(window)
    expect(root.style.getPropertyValue('--sheet-height')).toBe('554px')
    fireEvent.keyDown(screen.getByRole('button', { name: 'Change panel height (full)' }), {
        key: 'ArrowDown',
    })
    expect(root.style.getPropertyValue('--sheet-height')).toBe('326px')
    nearbyHeight = 184
    act(() => observed.get(nearby)!([], {} as ResizeObserver))
    expect(root.style.getPropertyValue('--sheet-height')).toBe('366px')
    fireEvent.keyDown(screen.getByRole('button', { name: 'Change panel height (half)' }), {
        key: 'ArrowDown',
    })
    expect(root.style.getPropertyValue('--sheet-height')).toBe('158px')
    fireEvent.keyDown(screen.getByRole('button', { name: 'Change panel height (collapsed)' }), {
        key: 'ArrowUp',
    })
    expect(root.style.getPropertyValue('--sheet-height')).toBe('366px')
})

it.each(['Half', 'Full'])(
    'opens the phone voice search from the %s menu on the mic click',
    (snap) => {
        class MapSpeech {
            static instances: MapSpeech[] = []
            static startCalls = 0
            lang = ''
            continuous = false
            interimResults = false
            maxAlternatives = 0
            onstart: (() => void) | null = null
            onresult: ((event: unknown) => void) | null = null
            onerror: ((event: unknown) => void) | null = null
            onend: (() => void) | null = null
            constructor() {
                MapSpeech.instances.push(this)
            }
            start() {
                // Deliberately does not fire onstart: the session is still waiting
                // for the permission decision, so no result exists yet.
                MapSpeech.startCalls++
            }
            stop() {
                this.onend?.()
            }
            abort() {}
        }
        vi.stubGlobal('SpeechRecognition', MapSpeech)
        const { fetcher } = app(`/?lang=en&snap=${snap.toLowerCase()}`, true)
        const input = screen.getByRole('textbox', { name: 'Search Positions' })
        expect(document.getElementById('map-shell')).toHaveAttribute(
            'data-snap',
            snap.toLowerCase(),
        )
        const searchesBefore = fetcher.mock.calls.filter(([input]) =>
            String(input).includes('/api/markers/search'),
        ).length
        fireEvent.click(screen.getByRole('button', { name: 'Start voice search' }))
        // The same click expands the menu and starts recognition once, with no wait
        // for a transcript or geolocation.
        expect(document.getElementById('map-shell')).toHaveAttribute('data-snap', 'full')
        expect(MapSpeech.startCalls).toBe(1)
        expect(screen.getByRole('group', { name: 'Voice search' })).toBeInTheDocument()
        expect(screen.getByRole('button', { name: 'Stop voice search' })).toBeInTheDocument()
        expect(screen.getByText('Waiting for microphone access…')).toBeInTheDocument()
        // The field itself is the same node, so recording cannot be cancelled by a remount.
        expect(screen.getByRole('textbox', { name: 'Search Positions' })).toBe(input)
        // No recognised text means no search request yet.
        expect(
            fetcher.mock.calls.filter(([call]) => String(call).includes('/api/markers/search'))
                .length,
        ).toBe(searchesBefore)
    },
)

it('keeps recognised phone voice text out of the search until the user stops', () => {
    class MapSpeech {
        static instances: MapSpeech[] = []
        lang = ''
        continuous = false
        interimResults = false
        maxAlternatives = 0
        onstart: (() => void) | null = null
        onresult: ((event: unknown) => void) | null = null
        onerror: ((event: unknown) => void) | null = null
        onend: (() => void) | null = null
        constructor() {
            MapSpeech.instances.push(this)
        }
        start() {
            this.onstart?.()
        }
        stop() {
            this.onend?.()
        }
        abort() {}
    }
    vi.stubGlobal('SpeechRecognition', MapSpeech)
    const { fetcher } = app('/?lang=en', true)
    fireEvent.click(screen.getByRole('button', { name: 'Start voice search' }))
    const instance = MapSpeech.instances[0]!
    const interimSearches = () =>
        fetcher.mock.calls.filter(([call]) => String(call).includes('/api/markers/search')).length
    const before = interimSearches()
    act(() =>
        instance.onresult?.({
            resultIndex: 0,
            results: Object.assign([{ isFinal: false, 0: { transcript: 'synthetic' } }], {
                length: 1,
            }),
        }),
    )
    expect(screen.getByText('synthetic')).toBeInTheDocument()
    // Interim words must never trigger a backend search.
    expect(interimSearches()).toBe(before)
    // Tapping Stop commits the complete text and runs the search once.
    fireEvent.click(screen.getByRole('button', { name: 'Stop voice search' }))
    expect(screen.getByRole('textbox', { name: 'Search Positions' })).toHaveValue('synthetic')
    return waitFor(() => expect(interimSearches()).toBeGreaterThan(before))
})

it('renders the half-menu Nursing card on two lines with no leading space', () => {
    const { container } = app('/?lang=en&snap=half', true)
    const card = container.querySelector('#mobile-nearby-nursing')!
    expect(card.querySelector('.card-title')!.textContent).toBe('Nursing\nRooms')
})

it('replaces the desktop nearby cards with the voice body while listening', () => {
    class DesktopSpeech {
        static instances: DesktopSpeech[] = []
        lang = ''
        continuous = false
        interimResults = false
        maxAlternatives = 0
        onstart: (() => void) | null = null
        onresult: ((event: unknown) => void) | null = null
        onerror: ((event: unknown) => void) | null = null
        onend: (() => void) | null = null
        constructor() {
            DesktopSpeech.instances.push(this)
        }
        start() {
            this.onstart?.()
        }
        stop() {
            this.onend?.()
        }
        abort() {}
    }
    vi.stubGlobal('SpeechRecognition', DesktopSpeech)
    const { container } = app('/?panel=search&lang=en')
    expect(screen.getByRole('button', { name: 'Nursing Rooms' })).toBeInTheDocument()
    const input = screen.getByRole('textbox', { name: 'Lycoris Maps' })
    fireEvent.click(screen.getByRole('button', { name: 'Start voice search' }))
    // The body replaces the cards instead of covering them.
    expect(screen.getByRole('group', { name: 'Voice search' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Nursing Rooms' })).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Stop voice search' })).toBeInTheDocument()
    // The desktop field is still the same node, so recording is not cancelled.
    expect(screen.getByRole('textbox', { name: 'Lycoris Maps' })).toBe(input)
    expect(container.querySelector('.desktop-panel .voice-search-body')).not.toBeNull()
})

it('resets the phone container voice state when the field is replaced by details', async () => {
    class ReplaceSpeech {
        static instances: ReplaceSpeech[] = []
        lang = ''
        continuous = false
        interimResults = false
        maxAlternatives = 0
        onstart: (() => void) | null = null
        onresult: ((event: unknown) => void) | null = null
        onerror: ((event: unknown) => void) | null = null
        onend: (() => void) | null = null
        constructor() {
            ReplaceSpeech.instances.push(this)
        }
        start() {
            this.onstart?.()
        }
        stop() {
            this.onend?.()
        }
        abort() {}
    }
    vi.stubGlobal('SpeechRecognition', ReplaceSpeech)
    app('/?lang=en', true)
    fireEvent.click(screen.getByRole('button', { name: 'Start voice search' }))
    expect(screen.getByRole('group', { name: 'Voice search' })).toBeInTheDocument()
    // Opening Nearby replaces the search field entirely.
    fireEvent.click(screen.getByRole('button', { name: 'Find nearby' }))
    await screen.findByRole('heading', { name: 'Nearby' })
    expect(screen.queryByRole('group', { name: 'Voice search' })).not.toBeInTheDocument()
    expect(document.getElementById('map-shell')).toHaveAttribute('data-snap', 'full')
})
