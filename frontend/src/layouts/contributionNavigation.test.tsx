import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter, useLocation, useNavigate } from 'react-router'
import { MapShell } from './MapShell'

function LocationProbe() {
    const location = useLocation()
    const navigate = useNavigate()
    return (
        <>
            <output data-testid="route">
                {location.pathname}
                {location.search}
                {location.hash}
            </output>
            <button onClick={() => void navigate(-1)}>Browser back</button>
            <button onClick={() => void navigate(1)}>Browser forward</button>
        </>
    )
}
function mount(initialEntry = '/maps?lang=en#kept') {
    return render(
        <MemoryRouter initialEntries={[initialEntry]}>
            <MapShell />
            <LocationProbe />
        </MemoryRouter>,
    )
}
function mockScreen(initialMobile: boolean) {
    let mobile = initialMobile
    const listeners = new Set<() => void>()
    vi.stubGlobal('matchMedia', () => ({
        get matches() {
            return mobile
        },
        addEventListener: (_type: string, fn: () => void) => listeners.add(fn),
        removeEventListener: (_type: string, fn: () => void) => listeners.delete(fn),
    }))
    return (value: boolean) =>
        act(() => {
            mobile = value
            listeners.forEach((fn) => fn())
        })
}
afterEach(() => {
    vi.unstubAllGlobals()
    vi.restoreAllMocks()
})

it('opens a desktop composer only after a real map click and keeps the selected point and map', async () => {
    const resize = mockScreen(false)
    const { container } = mount()
    const map = container.querySelector<HTMLElement>('.product-map')!
    const instance = map.dataset.mapId
    fireEvent.click(screen.getByRole('button', { name: /^Contribute$/ }))
    fireEvent.click(screen.getByText('Click on the map to add points.'))
    expect(screen.queryByRole('form', { name: 'Contribution draft' })).not.toBeInTheDocument()
    expect(map).toHaveAccessibleName(/press Enter/)
    fireEvent.click(map, { clientX: 650, clientY: 350 })
    const form = await screen.findByRole('form', { name: 'Contribution draft' })
    const point = [form.getAttribute('data-lat'), form.getAttribute('data-lng')]
    expect(point.every((value) => value !== null && Number.isFinite(Number(value)))).toBe(true)
    expect(screen.getByRole('heading', { name: 'Contribute' })).toHaveFocus()
    expect(map).toHaveAccessibleName('Map')
    fireEvent.change(screen.getByRole('textbox', { name: 'Title' }), {
        target: { value: 'Local draft' },
    })
    fireEvent.click(screen.getByRole('radio', { name: 'Nursing Rooms' }))
    resize(true)
    expect(screen.getByRole('textbox', { name: 'Title' })).toHaveValue('Local draft')
    expect(screen.getByRole('radio', { name: 'Nursing Rooms' })).toBeChecked()
    const mobileForm = screen.getByRole('form', { name: 'Contribution draft' })
    expect([mobileForm.getAttribute('data-lat'), mobileForm.getAttribute('data-lng')]).toEqual(
        point,
    )
    expect(container.querySelector('.product-map')).toBe(map)
    expect(map.dataset.mapId).toBe(instance)
    expect(screen.getByTestId('route')).toHaveTextContent(
        '/maps?lang=en&panel=contribute-form#kept',
    )
    fireEvent.click(screen.getByRole('button', { name: 'Close contribution form' }))
    await waitFor(() => expect(screen.getByTestId('route')).toHaveTextContent('/maps?lang=en#kept'))
    expect(screen.queryByRole('form', { name: 'Contribution draft' })).not.toBeInTheDocument()
})

it('picks a map location from the phone pen and preserves the draft after closing', async () => {
    mockScreen(true)
    const { container } = mount()
    const pen = screen.getByRole('button', { name: 'Contribute' })
    // jsdom has no layout; the return-focus guard needs the visible pen's rect.
    vi.spyOn(pen, 'getClientRects').mockReturnValue([
        new DOMRect(0, 0, 44, 44),
    ] as unknown as DOMRectList)
    fireEvent.click(pen)
    expect(screen.getByText('Click on the map to add points.')).toBeInTheDocument()
    expect(screen.queryByRole('form', { name: 'Contribution draft' })).not.toBeInTheDocument()
    fireEvent.click(container.querySelector('.product-map')!, { clientX: 200, clientY: 300 })
    expect(screen.queryByText('Click on the map to add points.')).not.toBeInTheDocument()
    expect(screen.getByRole('heading', { name: 'Contribute' })).toHaveFocus()
    fireEvent.change(screen.getByRole('textbox', { name: 'Title' }), {
        target: { value: 'Phone draft' },
    })
    const photo = new File(['test-only'], 'local-test.png', { type: 'image/png' })
    fireEvent.change(container.querySelector('input[type=file]')!, { target: { files: [photo] } })
    fireEvent.click(screen.getByRole('button', { name: 'Close contribution form' }))
    await waitFor(() => expect(screen.getByRole('button', { name: 'Contribute' })).toHaveFocus())
    expect(screen.getByTestId('route')).toHaveTextContent('/maps?lang=en#kept')
    fireEvent.click(screen.getByRole('button', { name: 'Contribute' }))
    fireEvent.click(container.querySelector('.product-map')!, { clientX: 220, clientY: 300 })
    expect(screen.getByRole('textbox', { name: 'Title' })).toHaveValue('Phone draft')
    expect(
        screen.getByRole('button', { name: 'Upload photo: local-test.png selected' }),
    ).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Submit' })).toHaveAttribute('aria-disabled', 'true')
})

it('keeps an active picker across screen sizes until the user chooses a point', async () => {
    const resize = mockScreen(false)
    mount()
    fireEvent.click(screen.getByRole('button', { name: /^Contribute$/ }))
    resize(true)
    expect(screen.queryByRole('form', { name: 'Contribution draft' })).not.toBeInTheDocument()
    expect(screen.getByText('Click on the map to add points.')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Close contribution mode' }))
    await waitFor(() => expect(screen.getByTestId('route')).toHaveTextContent('/maps?lang=en#kept'))
    expect(screen.queryByRole('form', { name: 'Contribution draft' })).not.toBeInTheDocument()
})

it('focuses search when clicking its outer frame', () => {
    mockScreen(false)
    mount()
    fireEvent.click(screen.getByRole('button', { name: /^Search$/ }))
    const input = screen.getByRole('textbox', { name: 'Lycoris Maps' })
    fireEvent.click(input.parentElement!)
    expect(input).toHaveFocus()
})

it('closes a composer opened from a direct picker URL without navigating out of the app', async () => {
    mockScreen(false)
    const { container } = mount('/maps?lang=en&panel=contribute#kept')
    fireEvent.keyDown(container.querySelector('.product-map')!, { key: 'Enter' })
    expect(await screen.findByRole('form', { name: 'Contribution draft' })).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Close contribution form' }))
    await waitFor(() => expect(screen.getByTestId('route')).toHaveTextContent('/maps?lang=en#kept'))
    expect(screen.queryByRole('form', { name: 'Contribution draft' })).not.toBeInTheDocument()
})

it('returns out of a desktop contribution in one Back after resizing to phone and restores the draft on Forward', async () => {
    const resize = mockScreen(false)
    const { container } = mount()
    fireEvent.click(screen.getByRole('button', { name: /^Contribute$/ }))
    fireEvent.click(container.querySelector('.product-map')!, { clientX: 650, clientY: 350 })
    fireEvent.change(await screen.findByRole('textbox', { name: 'Title' }), {
        target: { value: 'History draft' },
    })
    resize(true)
    fireEvent.click(screen.getByRole('button', { name: 'Browser back' }))
    await waitFor(() => expect(screen.getByTestId('route')).toHaveTextContent('/maps?lang=en#kept'))
    expect(screen.queryByRole('form', { name: 'Contribution draft' })).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Browser forward' }))
    expect(await screen.findByRole('textbox', { name: 'Title' })).toHaveValue('History draft')
})

it.each(['picker', 'form'])(
    'dismisses the desktop contribution %s without reopening Search',
    async (step) => {
        mockScreen(false)
        const { container } = mount('/maps?lang=en&markerId=123#kept')
        const map = container.querySelector('.product-map')
        fireEvent.click(screen.getByRole('button', { name: 'Search' }))
        fireEvent.click(screen.getByRole('button', { name: 'Contribute' }))
        if (step === 'form') fireEvent.click(map!, { clientX: 650, clientY: 350 })
        fireEvent.click(
            screen.getByRole('button', {
                name: step === 'form' ? 'Close contribution form' : 'Close contribution mode',
            }),
        )
        await waitFor(() =>
            expect(screen.getByTestId('route').textContent).toBe('/maps?lang=en&markerId=123#kept'),
        )
        expect(screen.queryByRole('heading', { name: 'Search' })).not.toBeInTheDocument()
        expect(screen.queryByRole('form', { name: 'Contribution draft' })).not.toBeInTheDocument()
        expect(screen.queryByText('Click on the map to add points.')).not.toBeInTheDocument()
        expect(container.querySelector('.product-map')).toBe(map)
    },
)
