import { useEffect, useState } from 'react'
import { useAdminUi } from './ui'
import type { QueueEntry } from './undoQueue'

export function UndoNotifications({
    entries,
    undo,
    dismiss,
}: {
    entries: QueueEntry[]
    undo: (id: string) => boolean
    dismiss: (id: string) => void
}) {
    const ui = useAdminUi()
    const [, tick] = useState(0)
    useEffect(() => {
        if (!entries.some((entry) => entry.status === 'pending')) return
        const timer = window.setInterval(() => tick((value) => value + 1), 250)
        return () => window.clearInterval(timer)
    }, [entries])
    const pending = entries.filter((entry) => entry.status === 'pending')
    const shown = pending.slice(-3)
    const extra = pending.length - shown.length
    const results = entries.filter((entry) => entry.status === 'failed' || entry.status === 'done')
    if (!pending.length && !results.length) return null
    return (
        <div className="review-notifications">
            {shown.map((entry) => (
                <div
                    key={entry.id}
                    className="review-notification"
                    role="status"
                    aria-live="polite"
                >
                    <span>
                        {ui.message(entry.decision === 'approve' ? 'Approved' : 'Rejected')} ·{' '}
                        {entry.items.length > 1
                            ? `${entry.items.length} ${ui.message('items')}`
                            : `《${entry.items[0]?.title}》`}
                    </span>
                    <span className="review-countdown">
                        {Math.max(0, Math.ceil((entry.deadline - Date.now()) / 1000))}s
                    </span>
                    <button type="button" className="review-undo" onClick={() => undo(entry.id)}>
                        {ui.message('Undo')}
                    </button>
                </div>
            ))}
            {extra > 0 && (
                <div className="review-notification" role="status" aria-live="polite">
                    {extra} {ui.message('more items pending submission')}
                </div>
            )}
            {results.map((entry) =>
                entry.status === 'failed' ? (
                    <div key={entry.id} className="review-notification review-failure" role="alert">
                        <span>
                            {entry.items.length > 1
                                ? `${entry.items.length} ${ui.message('items')}`
                                : `《${entry.items[0]?.title}》`}
                            :{' '}
                            {ui.message(
                                entry.error ?? 'The request failed. Refresh and try again.',
                            )}
                        </span>
                        <button type="button" onClick={() => dismiss(entry.id)}>
                            {ui.message('Dismiss')}
                        </button>
                    </div>
                ) : (
                    entry.summary && (
                        <div
                            key={entry.id}
                            className="review-notification review-summary"
                            role={entry.summary.failed ? 'alert' : 'status'}
                        >
                            <span>
                                {ui.message('Succeeded')} {entry.summary.ok} {ui.message('items')},{' '}
                                {ui.message('Failed')} {entry.summary.failed} {ui.message('items')}
                            </span>
                            {entry.summary.failures.length > 0 && (
                                <details>
                                    <summary>{ui.message('Show failure reasons')}</summary>
                                    <ul>
                                        {entry.summary.failures.map((failure, index) => (
                                            <li key={index}>{ui.message(failure)}</li>
                                        ))}
                                    </ul>
                                </details>
                            )}
                            <button type="button" onClick={() => dismiss(entry.id)}>
                                {ui.message('Dismiss')}
                            </button>
                        </div>
                    )
                ),
            )}
        </div>
    )
}
