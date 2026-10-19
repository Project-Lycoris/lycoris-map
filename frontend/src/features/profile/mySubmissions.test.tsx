import { cleanup, render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { LanguageProvider } from '@/shared/i18n'
import * as submissionsApi from '@/shared/api/mySubmissions'
import { MySubmissions } from './MySubmissions'

vi.mock('@/features/auth/SessionProvider', async (importOriginal) => {
    const actual = await importOriginal<typeof import('@/features/auth/SessionProvider')>()
    return {
        ...actual,
        useSession: () => ({
            store: {
                runPrivate: (
                    _scope: unknown,
                    operation: (signal: AbortSignal) => unknown,
                    signal: AbortSignal,
                ) => operation(signal),
            },
            scope: { publicId: 'A', authEpoch: 1 },
            user: { publicId: 'A' },
            busy: false,
        }),
    }
})

let client: QueryClient
beforeEach(() => {
    client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
})
afterEach(() => {
    cleanup()
    client.clear()
    vi.restoreAllMocks()
})

function mount() {
    return render(
        <QueryClientProvider client={client}>
            <LanguageProvider initialPreference="en">
                <MySubmissions />
            </LanguageProvider>
        </QueryClientProvider>,
    )
}

it('renders every submission status and a rejection reason', async () => {
    vi.spyOn(submissionsApi, 'readMySubmissions').mockResolvedValue([
        {
            kind: 'marker',
            id: 1,
            markerId: 1,
            markerTitle: 'Approved place',
            status: 'APPROVED',
            reviewNote: null,
            createdAt: '2026-10-18T06:50:15Z',
            reviewedAt: '2026-10-18T07:00:00Z',
        },
        {
            kind: 'edit',
            id: 2,
            markerId: 1,
            markerTitle: 'Edited place',
            status: 'PENDING',
            reviewNote: null,
            createdAt: '2026-10-18T06:50:15Z',
            reviewedAt: null,
        },
        {
            kind: 'image',
            id: 3,
            markerId: 1,
            markerTitle: 'Photo place',
            status: 'REJECTED',
            reviewNote: 'blurry',
            createdAt: '2026-10-18T06:50:15Z',
            reviewedAt: '2026-10-18T07:00:00Z',
        },
    ])
    mount()
    const section = await screen.findByRole('region', { name: 'My submissions' })
    expect(screen.getByText('Approved place')).toBeInTheDocument()
    expect(screen.getByText('Edited place')).toBeInTheDocument()
    expect(screen.getByText('Photo place')).toBeInTheDocument()
    expect(screen.getByText('Not approved: blurry')).toBeInTheDocument()
    expect(section).toBeInTheDocument()
})

it('hides entirely when the route is missing', async () => {
    vi.spyOn(submissionsApi, 'readMySubmissions').mockResolvedValue(null)
    const { container } = mount()
    await waitFor(() => expect(container).toBeEmptyDOMElement())
})

it('stays hidden and silent on a real error', async () => {
    vi.spyOn(submissionsApi, 'readMySubmissions').mockRejectedValue(new Error('boom'))
    const { container } = mount()
    await waitFor(() => expect(container).toBeEmptyDOMElement())
})
