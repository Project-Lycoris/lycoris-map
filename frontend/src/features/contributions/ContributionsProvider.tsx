import {
    createContext,
    useContext,
    useEffect,
    useState,
    useSyncExternalStore,
    type ReactNode,
} from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useSessionStore } from '@/features/auth/SessionProvider'
import { BrowserDraftJournal } from './DraftJournal'
import { ContributionStore } from './ContributionStore'
const Context = createContext<ContributionStore | null>(null)
export function ContributionsProvider({ children }: { children: ReactNode }) {
    const client = useQueryClient(),
        session = useSessionStore()!
    const [store] = useState(
        () =>
            new ContributionStore(
                client,
                session,
                undefined,
                undefined,
                undefined,
                new BrowserDraftJournal(),
            ),
    )
    useEffect(() => {
        const unsubscribe = session.subscribe(store.clearStale)
        store.clearStale()
        return () => {
            unsubscribe()
            store.dispose()
        }
    }, [session, store])
    return <Context value={store}>{children}</Context>
}
const noop = () => () => undefined
const empty = () => null
export function useContributions() {
    const store = useContext(Context)
    const state = useSyncExternalStore(store?.subscribe ?? noop, store?.getSnapshot ?? empty)
    return { store, state }
}
