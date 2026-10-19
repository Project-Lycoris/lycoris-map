import type { QueryClient } from '@tanstack/react-query'
import { privateKeys, publicKeys, type PrivateScope } from '@/shared/query/keys'

/** Refresh the dashboard after writes; other map/account data refreshes when opened. */
export async function refreshModeration(
    client: QueryClient,
    prefix: readonly unknown[],
    scope: PrivateScope | null,
) {
    await Promise.all([
        client.invalidateQueries({
            queryKey: prefix,
            predicate: (query) => query.queryKey.at(-1) !== 'access',
        }),
        scope &&
            client.invalidateQueries({
                queryKey: privateKeys.scope(scope),
                predicate: (query) => !query.queryKey.includes('admin'),
                refetchType: 'none',
            }),
        client.invalidateQueries({ queryKey: publicKeys.markers(), refetchType: 'none' }),
    ])
}
