import { useEffect, useState } from 'react'
import { useSearchParams } from 'react-router'
import { useQuery } from '@tanstack/react-query'
import { DesignButton } from '@/shared/ui/design-primitives'
import { useAdminWork } from './work'
import { useAdminUi } from './ui'
import * as api from './api'
import { Pagination } from './Moderation'
export function Users() {
    const work = useAdminWork(),
        ui = useAdminUi(),
        [params] = useSearchParams(),
        [search, setSearch] = useState(() => params.get('q') ?? ''),
        [filter, setFilter] = useState({ q: '', page: 0 })
    // Audit-log links arrive with `?q=<publicId>`; keep the field in sync.
    useEffect(() => {
        const fromUrl = params.get('q')
        if (fromUrl !== null && fromUrl !== search) setSearch(fromUrl)
    }, [params])
    useEffect(() => {
        const timer = setTimeout(() => setFilter({ q: search.trim(), page: 0 }), 300)
        return () => clearTimeout(timer)
    }, [search])
    const query = useQuery({
        queryKey: [...work.prefix, 'users', filter],
        retry: false,
        gcTime: 0,
        queryFn: ({ signal }) => work.run((s) => api.readUsers(filter.page, filter.q, s), signal),
    })
    const total = Math.max(1, query.data?.totalPages ?? 1)
    useEffect(() => {
        if (query.data && filter.page >= total)
            setFilter((previous) => ({ ...previous, page: total - 1 }))
    }, [query.data, filter.page, total])
    const action = (user: api.AdminUser, kind: 'disable' | 'restore' | 'reset-password') =>
        work.confirm({
            label: `${ui.message(kind === 'disable' ? 'Disable' : kind === 'restore' ? 'Restore' : 'Reset password')} · ${user.username || user.publicId} #${user.id}`,
            detail: ui.message(
                kind === 'disable'
                    ? 'This disables the account and invalidates its sessions.'
                    : kind === 'reset-password'
                      ? 'This resets the password to the server-configured default and invalidates existing sessions.'
                      : 'Restore this account?',
            ),
            action: (signal) => api.changeUser(user.id, kind, signal),
        })
    return (
        <section className="admin-content">
            <div className="admin-toolbar">
                <label className="admin-field">
                    {ui.message('Search users')}
                    <input
                        type="search"
                        value={search}
                        onChange={(e) => setSearch(e.target.value)}
                        maxLength={255}
                    />
                </label>
                <DesignButton
                    disabled={work.busy || query.isFetching}
                    onClick={() => void query.refetch()}
                >
                    {ui.message('Refresh')}
                </DesignButton>
            </div>
            <p role="status">
                {ui.message(
                    query.isError
                        ? 'The request failed. Refresh and try again.'
                        : query.isFetching
                          ? 'Please wait…'
                          : !query.data?.items.length
                            ? 'No items.'
                            : '',
                )}
            </p>
            {!query.isError && query.data && (
                <div className="admin-list">
                    {query.data.items.map((user) => (
                        <article className="admin-card" key={user.id}>
                            <h2>{user.nickname || user.username || user.publicId}</h2>
                            <p>
                                {user.username || user.publicId} · #{user.id}
                            </p>
                            <p>{user.email}</p>
                            <p>
                                {ui.message(user.role === 'ADMIN' ? 'Admin' : 'User')} ·{' '}
                                {ui.message(user.deleted ? 'Disabled' : 'Active')}
                            </p>
                            <div className="admin-actions">
                                <DesignButton
                                    disabled={
                                        work.busy ||
                                        (!user.deleted &&
                                            user.publicId === work.session.user?.publicId)
                                    }
                                    onClick={() =>
                                        action(user, user.deleted ? 'restore' : 'disable')
                                    }
                                >
                                    {ui.message(user.deleted ? 'Restore' : 'Disable')}
                                </DesignButton>
                                <DesignButton
                                    disabled={work.busy || user.deleted}
                                    onClick={() => action(user, 'reset-password')}
                                >
                                    {ui.message('Reset password')}
                                </DesignButton>
                            </div>
                        </article>
                    ))}
                </div>
            )}
            <Pagination
                page={filter.page}
                total={total}
                busy={work.busy || query.isFetching}
                change={(page) => setFilter({ ...filter, page })}
            />
        </section>
    )
}
