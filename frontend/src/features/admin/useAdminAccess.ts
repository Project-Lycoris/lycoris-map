import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useSession } from '@/features/auth/SessionProvider'
import { privateKeys, type PrivateScope } from '@/shared/query/keys'
import { ApiError } from '@/shared/api/ApiError'
import { readUsers } from './api'
export type Access = 'ready' | 'denied' | 'unavailable'
export function deniedAccess(error: unknown): Exclude<Access, 'ready'> | null {
    if (!(error instanceof ApiError) || error.status !== 403) return null
    if (error.accessDenied) return 'denied'
    return 'unavailable'
}
export const adminKeys = (scope: PrivateScope) => [...privateKeys.scope(scope), 'admin'] as const
export type AdminRun = <T>(
    operation: (signal: AbortSignal) => Promise<T>,
    signal?: AbortSignal,
) => Promise<T>
export function useAdminAccess() {
    const session = useSession(),
        client = useQueryClient(),
        scope = session.scope,
        store = session.store
    const prefix = scope ? adminKeys(scope) : ['admin-anonymous']
    const accessKey = [...prefix, 'access']
    const access = useQuery({
        queryKey: accessKey,
        enabled: !!scope && !session.busy,
        retry: false,
        gcTime: 0,
        staleTime: 0,
        refetchOnWindowFocus: true,
        refetchOnReconnect: true,
        queryFn: async ({ signal }): Promise<Access> => {
            try {
                await store!.runPrivate(scope!, (s) => readUsers(0, '', s, 1), signal)
                return 'ready'
            } catch (error) {
                const denial = deniedAccess(error)
                if (denial) return denial
                throw error
            }
        },
    })
    const run: AdminRun = async (operation, signal) => {
        if (!scope || !store) throw new DOMException('Session unavailable', 'AbortError')
        try {
            return await store.runPrivate(scope, operation, signal)
        } catch (error) {
            const denial = deniedAccess(error)
            if (store.isCurrent(scope) && denial) {
                // Purge sensitive data as soon as authorization expires; retain only gate state.
                await client.cancelQueries({ queryKey: prefix })
                client.removeQueries({
                    queryKey: prefix,
                    predicate: (query) => query.queryKey.at(-1) !== 'access',
                })
                client.setQueryData(accessKey, denial)
            }
            throw error
        }
    }
    return { access, run, session, prefix }
}
