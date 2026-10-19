import { useQuery } from '@tanstack/react-query'
import { useAdminUi } from './ui'
import { useAdminWork } from './work'
import * as api from './api'
import { fullTime, relativeTime } from './time'
import { localizedReason } from './reason'

/** Per-place moderation history; tolerates a backend that lacks the route. */
export function ModerationTimeline({ markerId }: { markerId: number }) {
    const ui = useAdminUi(),
        work = useAdminWork(),
        query = useQuery({
            queryKey: [...work.prefix, 'events', markerId],
            gcTime: 0,
            retry: false,
            queryFn: ({ signal }) =>
                work.run(
                    (s) => api.readModerationEvents({ page: 0, size: 20, markerId }, s),
                    signal,
                ),
        })
    if (query.isError && api.isMissingRoute(query.error))
        return (
            <p className="admin-meta">
                {ui.message('Server update required to view moderation history')}
            </p>
        )
    return (
        <section className="moderation-timeline" aria-label={ui.message('Moderation history')}>
            <h3>{ui.message('Moderation history')}</h3>
            {query.isPending && <p role="status">{ui.message('Loading places…')}</p>}
            {query.isError && (
                <p role="alert">
                    {ui.message('The request failed. Refresh and try again.')}{' '}
                    <button type="button" onClick={() => void query.refetch()}>
                        {ui.message('Retry')}
                    </button>
                </p>
            )}
            {query.data && !query.data.items.length && <p>{ui.message('No items.')}</p>}
            {query.data && !!query.data.items.length && (
                <ol>
                    {query.data.items.map((event) => (
                        <li key={event.id}>
                            <p className="timeline-line">
                                {event.actorUsername} ·{' '}
                                {ui.message(api.moderationActionLabel(event.action))} ·{' '}
                                <time
                                    dateTime={event.createdAt}
                                    title={fullTime(event.createdAt, ui.language)}
                                >
                                    {relativeTime(event.createdAt, ui.language)}
                                </time>
                            </p>
                            {event.note && (
                                <p className="admin-meta">
                                    {localizedReason(event.note, ui.message)}
                                </p>
                            )}
                        </li>
                    ))}
                </ol>
            )}
        </section>
    )
}
