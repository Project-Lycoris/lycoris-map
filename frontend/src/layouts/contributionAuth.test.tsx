import { StrictMode } from 'react'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter, useLocation, useNavigate } from 'react-router'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { AppProviders } from '@/app/providers'
import { MapPage } from '@/app/MapPage'
import { useSession } from '@/features/auth/SessionProvider'
import * as api from '@/shared/api/session'
import * as writes from '@/shared/api/markerWrites'
import type { User } from '@/shared/api/users'

const account: User = {
    publicId: 'A',
    username: 'Synthetic',
    email: null,
    nickname: null,
    avatarUrl: null,
    pronouns: null,
    signature: null,
}
let server: User | null
beforeEach(() => {
    server = null
    localStorage.clear()
    vi.spyOn(api, 'fetchMe').mockImplementation(async () => server)
    vi.spyOn(api, 'login').mockImplementation(async () => (server = account))
    vi.spyOn(writes, 'createMarker')
    vi.stubGlobal(
        'fetch',
        vi.fn(async () => new Response('[]', { headers: { 'Content-Type': 'application/json' } })),
    )
})
afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
    localStorage.clear()
})

function mount(mobile: boolean, path = '/?lang=en') {
    vi.stubGlobal('matchMedia', () => ({
        matches: mobile,
        addEventListener: () => {},
        removeEventListener: () => {},
    }))
    let leave = () => {}
    function Probe() {
        const session = useSession(),
            location = useLocation(),
            navigate = useNavigate()
        leave = () => {
            void navigate('/?lang=en&panel=settings')
        }
        return (
            <>
                <output data-testid="session">{session.status}</output>
                <output data-testid="route">{location.search}</output>
            </>
        )
    }
    const view = render(
        <StrictMode>
            <MemoryRouter initialEntries={[path]}>
                <AppProviders languageOverride="en">
                    <MapPage />
                    <Probe />
                </AppProviders>
            </MemoryRouter>
        </StrictMode>,
    )
    return { ...view, leave: () => act(leave) }
}
async function login() {
    fireEvent.change(await screen.findByLabelText('Email or Username'), {
        target: { value: 'Synthetic' },
    })
    const password = screen.getByLabelText('Password', { exact: true })
    fireEvent.change(password, { target: { value: 'test-password' } })
    fireEvent.submit(password.closest('form')!)
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
}

it.each([false, true])(
    'requires login before contributing, then resumes the correct flow (mobile=%s)',
    async (mobile) => {
        const view = mount(mobile)
        await waitFor(() => expect(screen.getByTestId('session')).toHaveTextContent('anonymous'))
        const map = view.container.querySelector('.leaflet-container')
        fireEvent.click(screen.getByRole('button', { name: 'Contribute' }))
        expect(await screen.findByRole('dialog', { name: 'Login' })).toBeInTheDocument()
        expect(view.container.querySelector('.contribution-bar')).toBeNull()
        expect(view.container.querySelector('.contribution-form')).toBeNull()
        expect(writes.createMarker).not.toHaveBeenCalled()
        await login()
        expect(screen.getByText('Click on the map to add points.')).toBeInTheDocument()
        expect(screen.queryByRole('form', { name: 'Contribution draft' })).not.toBeInTheDocument()
        fireEvent.click(view.container.querySelector('.product-map')!, {
            clientX: 200,
            clientY: 300,
        })
        expect(await screen.findByRole('form', { name: 'Contribution draft' })).toBeInTheDocument()
        expect(view.container.querySelector('.leaflet-container')).toBe(map)
        expect(writes.createMarker).not.toHaveBeenCalled()
    },
)
it.each([false, true])(
    'cancelling login exits contribution without prompting again (mobile=%s)',
    async (mobile) => {
        mount(mobile)
        fireEvent.click(screen.getByRole('button', { name: 'Contribute' }))
        await screen.findByRole('dialog', { name: 'Login' })
        fireEvent.click(screen.getByRole('button', { name: 'Close account window' }))
        await waitFor(() => expect(screen.getByTestId('route')).not.toHaveTextContent('contribute'))
        expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
        expect(screen.queryByRole('form', { name: 'Contribution draft' })).not.toBeInTheDocument()
        fireEvent.click(screen.getByRole('button', { name: 'Contribute' }))
        expect(await screen.findByRole('dialog', { name: 'Login' })).toBeInTheDocument()
    },
)
it.each(['contribute', 'contribute-form'])(
    'protects a direct %s URL and resumes it after login',
    async (panel) => {
        mount(false, `/?lang=en&panel=${panel}`)
        expect(await screen.findByRole('dialog', { name: 'Login' })).toBeInTheDocument()
        await login()
        expect(document.getElementById('map-shell')).toHaveAttribute('data-panel', 'contribute')
    },
)
it('does not close a different route when the user leaves during login', async () => {
    const view = mount(false)
    fireEvent.click(screen.getByRole('button', { name: 'Contribute' }))
    await screen.findByRole('dialog', { name: 'Login' })
    view.leave()
    fireEvent.click(screen.getByRole('button', { name: 'Close account window' }))
    expect(screen.getByTestId('route')).toHaveTextContent('panel=settings')
})
it('waits for the initial session check instead of prompting a returning signed-in user', async () => {
    let resolve!: (user: User | null) => void
    const checked = new Promise<User | null>((r) => {
        resolve = r
    })
    vi.mocked(api.fetchMe).mockReturnValue(checked)
    mount(false, '/?lang=en&panel=contribute')
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
    await waitFor(() => expect(resolve).toBeTypeOf('function'))
    await act(async () => resolve(account))
    expect(await screen.findByText('Click on the map to add points.')).toBeInTheDocument()
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
})
