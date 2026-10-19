import { useNavigate, useSearchParams } from 'react-router'
import { useQuery } from '@tanstack/react-query'
import { DesignButton } from '@/shared/ui/design-primitives'
import { useAdminUi } from './ui'
import { useAdminWork } from './work'
import * as api from './api'
import { Pagination } from './Pagination'
import { fullTime, relativeTime } from './time'
import { localizedReason } from './reason'

export function AuditLog() {
    const ui = useAdminUi(),
        work = useAdminWork(),
        navigate = useNavigate(),
        [params, setParams] = useSearchParams()
    const page = Math.max(0, Number(params.get('page') ?? '0') || 0)
    const query = useQuery({
        queryKey: [...work.prefix, 'events', 'log', page],
        gcTime: 0,
        retry: false,
        queryFn: ({ signal }) =>
            work.run((s) => api.readModerationEvents({ page, size: 50 }, s), signal),
    })
    const missing = query.isError && api.isMissingRoute(query.error)
    const totalPages = Math.max(1, query.data?.totalPages ?? 1)
    const open = (event: api.ModerationEvent) => {
        if (event.targetKind === 'user') navigate(`/admin/usr?q=${event.targetId}`)
        else navigate(`/admin/all?markerId=${event.markerId ?? event.targetId}`)
    }
    return (
        <section className="admin-content audit-log">
            <p role="status">
                {ui.message(
                    missing
                        ? 'Server update required to view moderation history'
                        : query.isError
                          ? 'The request failed. Refresh and try again.'
                          : query.isFetching
                            ? 'Loading places…'
                            : !query.data?.items.length
                              ? 'No items.'
                              : '',
                )}
            </p>
            {query.data && !!query.data.items.length && (
                <ul className="audit-list" aria-label={ui.message('Moderation history')}>
                    {query.data.items.map((event) => (
                        <li key={event.id}>
                            <button type="button" onClick={() => open(event)}>
                                <span className="audit-actor">{event.actorUsername}</span>
                                <span className="audit-action">
                                    {ui.message(api.moderationActionLabel(event.action))}
                                </span>
                                <span className="audit-target">
                                    {ui.message(api.moderationTargetLabel(event.targetKind))}
                                    {event.targetKind === 'user'
                                        ? event.targetLabel
                                            ? ` · ${event.targetLabel}`
                                            : ''
                                        : event.markerTitle
                                          ? ` · ${event.markerTitle}`
                                          : ''}
                                </span>
                                <time
                                    dateTime={event.createdAt}
                                    title={fullTime(event.createdAt, ui.language)}
                                >
                                    {relativeTime(event.createdAt, ui.language)}
                                </time>
                                {event.note && (
                                    <span className="audit-note">
                                        {localizedReason(event.note, ui.message)}
                                    </span>
                                )}
                            </button>
                        </li>
                    ))}
                </ul>
            )}
            <div className="all-places-pager">
                <Pagination
                    page={Math.min(page, totalPages - 1)}
                    total={totalPages}
                    busy={query.isFetching}
                    change={(value) =>
                        setParams(
                            (current) => {
                                const next = new URLSearchParams(current)
                                if (value <= 0) next.delete('page')
                                else next.set('page', String(value))
                                return next
                            },
                            { replace: true },
                        )
                    }
                />
                <p>
                    {Math.min(page, totalPages - 1) + 1} / {totalPages} ·{' '}
                    {query.data?.totalElements ?? 0} {ui.message('items')}
                </p>
            </div>
            {!missing && query.isError && (
                <DesignButton onClick={() => void query.refetch()}>
                    {ui.message('Retry')}
                </DesignButton>
            )}
        </section>
    )
}
