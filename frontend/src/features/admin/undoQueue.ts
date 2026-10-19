import { useCallback, useEffect, useRef, useState } from 'react'
import { ApiError } from '@/shared/api/ApiError'
import type { Language } from '@/shared/i18n'
import * as api from './api'
import type { ReviewItem } from './reviewModel'
import type { AdminRun } from './useAdminAccess'

export type QueueEntry = {
    id: string
    items: ReviewItem[]
    decision: 'approve' | 'reject'
    reason: string | undefined
    skipped: number
    batch: boolean
    deadline: number
    status: 'pending' | 'sending' | 'failed' | 'done'
    error: string | undefined
    summary: { ok: number; failed: number; failures: string[]; skipped: number } | undefined
}

export type ScheduleInput = {
    items: ReviewItem[]
    decision: 'approve' | 'reject'
    reason?: string | undefined
    /** Out-of-date proposals that were skipped before submission. */
    skipped?: number | undefined
    /** Keep the batch endpoint even when only one item remains. */
    batch?: boolean | undefined
}

export type QueueSummary = { ok: number; failed: number; failures: string[]; skipped: number }

const plural: Record<ReviewItem['kind'], 'markers' | 'edits' | 'images'> = {
    marker: 'markers',
    edit: 'edits',
    image: 'images',
}

export function moderationError(error: unknown): string {
    if (error instanceof ApiError && error.status === 409)
        return 'The item changed. Review the refreshed list.'
    if (!(error instanceof ApiError) || error.status === 0 || error.status >= 500)
        return 'The outcome could not be confirmed. Refresh and inspect the item before trying again.'
    return 'The request failed. Refresh and try again.'
}

function singlePath(entry: QueueEntry, language: Language): string {
    const item = entry.items[0]!
    return `${api.moderationPath(plural[item.kind], item.id, entry.decision)}?lang=${language}`
}

function singleBody(entry: QueueEntry): api.ModerationPayload | undefined {
    const body: api.ModerationPayload = {}
    if (entry.reason?.trim()) body.reason = entry.reason.trim()
    return Object.keys(body).length ? body : undefined
}

function batchBody(entry: QueueEntry, items: ReviewItem[]) {
    return {
        items: items.map((item) => ({
            kind: item.kind,
            id: item.id,
            decision: entry.decision,
            ...(entry.reason?.trim() ? { reason: entry.reason.trim() } : {}),
        })),
    }
}

/**
 * `request` cannot use `keepalive` (its deadline adapter aborts the body read),
 * so page-hide flushes go through a deliberately small fetch that keeps the
 * same-origin Origin header, cookies, JSON headers and contract paths.
 */
export function sendKeepalive(requests: QueueEntry[], language: Language) {
    for (const entry of requests) {
        if (entry.items.length === 1 && !entry.batch) {
            const body = singleBody(entry)
            void fetch(singlePath(entry, language), {
                method: 'POST',
                credentials: 'include',
                keepalive: true,
                headers: {
                    Accept: 'application/json, text/plain, */*',
                    ...(body ? { 'Content-Type': 'application/json' } : {}),
                },
                ...(body ? { body: JSON.stringify(body) } : {}),
            }).catch(() => undefined)
        } else {
            // Navigation must obey the same 50-item API limit as normal saves.
            for (let offset = 0; offset < entry.items.length; offset += 50) {
                const body = batchBody(entry, entry.items.slice(offset, offset + 50))
                void fetch('/api/admin/moderation/batch', {
                    method: 'POST',
                    credentials: 'include',
                    keepalive: true,
                    headers: {
                        Accept: 'application/json, text/plain, */*',
                        'Content-Type': 'application/json',
                    },
                    body: JSON.stringify(body),
                }).catch(() => undefined)
            }
        }
    }
}

export function useUndoQueue({
    run,
    language,
    onRestore,
    onCommit,
}: {
    run: AdminRun
    language: Language
    onRestore: (items: ReviewItem[]) => void
    onCommit: (items: ReviewItem[]) => void
}) {
    const [version, setVersion] = useState(0)
    const entries = useRef<QueueEntry[]>([])
    const timers = useRef(new Map<string, number>())
    const counter = useRef(0)
    const runRef = useRef(run)
    const languageRef = useRef(language)
    runRef.current = run
    languageRef.current = language

    const bump = useCallback(() => setVersion((value) => value + 1), [])

    const drop = useCallback(
        (id: string) => {
            const timer = timers.current.get(id)
            if (timer !== undefined) window.clearTimeout(timer)
            timers.current.delete(id)
            entries.current = entries.current.filter((entry) => entry.id !== id)
            bump()
        },
        [bump],
    )

    const execute = useCallback(
        async (entry: QueueEntry) => {
            if (entry.status !== 'pending') return
            timers.current.delete(entry.id)
            entry.status = 'sending'
            bump()
            const fail = (error: unknown) => {
                entry.status = 'failed'
                entry.error = moderationError(error)
                bump()
                onRestore(entry.items)
            }
            if (entry.items.length === 1 && !entry.batch) {
                const item = entry.items[0]!
                try {
                    await runRef.current(
                        (signal) =>
                            api.moderate(
                                plural[item.kind],
                                item.id,
                                entry.decision,
                                languageRef.current,
                                signal,
                                { reason: entry.reason },
                            ),
                        undefined,
                    )
                    drop(entry.id)
                    onCommit(entry.items)
                } catch (error) {
                    fail(error)
                }
                return
            }
            try {
                const failures: string[] = []
                const okItems: ReviewItem[] = []
                const failedItems: ReviewItem[] = []
                for (let offset = 0; offset < entry.items.length; offset += 50) {
                    const chunk = entry.items.slice(offset, offset + 50)
                    let results: api.BatchResult[]
                    try {
                        const response = await runRef.current(
                            (signal) =>
                                api.batchModerate(
                                    chunk.map((item) => ({
                                        kind: item.kind,
                                        id: item.id,
                                        decision: entry.decision,
                                        ...(entry.reason?.trim()
                                            ? { reason: entry.reason.trim() }
                                            : {}),
                                    })),
                                    signal,
                                ),
                            undefined,
                        )
                        results = response.results
                    } catch (error) {
                        failures.push(moderationError(error))
                        failedItems.push(...chunk)
                        continue
                    }
                    const failedKeys = new Set<string>()
                    for (const result of results) {
                        if (result.ok)
                            okItems.push(
                                ...chunk.filter(
                                    (item) => item.key === `${result.kind}-${result.id}`,
                                ),
                            )
                        else {
                            failedKeys.add(`${result.kind}-${result.id}`)
                            failures.push(
                                result.message ||
                                    moderationError(
                                        new ApiError(result.status ?? 0, result.message ?? ''),
                                    ),
                            )
                        }
                    }
                    failedItems.push(...chunk.filter((item) => failedKeys.has(item.key)))
                }
                entry.status = 'done'
                entry.summary = {
                    ok: okItems.length,
                    failed: failedItems.length,
                    failures,
                    skipped: entry.skipped,
                }
                bump()
                if (okItems.length) onCommit(okItems)
                if (failedItems.length) onRestore(failedItems)
            } catch (error) {
                entry.status = 'failed'
                entry.error = moderationError(error)
                bump()
                onRestore(entry.items)
            }
        },
        [bump, drop, onCommit, onRestore],
    )

    const schedule = useCallback(
        (input: ScheduleInput) => {
            const entry: QueueEntry = {
                id: `op-${++counter.current}`,
                items: input.items,
                decision: input.decision,
                reason: input.reason,
                skipped: input.skipped ?? 0,
                batch: input.batch ?? false,
                deadline: Date.now() + 5000,
                status: 'pending',
                error: undefined,
                summary: undefined,
            }
            entries.current = [...entries.current, entry]
            bump()
            timers.current.set(
                entry.id,
                window.setTimeout(() => void execute(entry), 5000),
            )
            return entry.id
        },
        [bump, execute],
    )

    const undo = useCallback(
        (id: string) => {
            const entry = entries.current.find((value) => value.id === id)
            if (!entry || entry.status !== 'pending') return false
            drop(id)
            onRestore(entry.items)
            return true
        },
        [drop, onRestore],
    )

    const undoLatest = useCallback(() => {
        const entry = [...entries.current].reverse().find((value) => value.status === 'pending')
        return entry ? undo(entry.id) : false
    }, [undo])

    const flush = useCallback(() => {
        const pending = entries.current.filter((entry) => entry.status === 'pending')
        if (!pending.length) return
        for (const entry of pending) {
            const timer = timers.current.get(entry.id)
            if (timer !== undefined) window.clearTimeout(timer)
            timers.current.delete(entry.id)
            entry.status = 'sending'
        }
        entries.current = entries.current.filter((entry) => !pending.includes(entry))
        sendKeepalive(pending, languageRef.current)
        bump()
    }, [bump])

    useEffect(() => {
        const hide = () => flush()
        window.addEventListener('pagehide', hide)
        return () => {
            window.removeEventListener('pagehide', hide)
            flush()
        }
    }, [flush])

    const report = useCallback(
        (summary: QueueSummary) => {
            entries.current = [
                ...entries.current,
                {
                    id: `op-${++counter.current}`,
                    items: [],
                    decision: 'reject',
                    reason: undefined,
                    skipped: summary.skipped,
                    batch: false,
                    deadline: Date.now(),
                    status: 'done',
                    error: undefined,
                    summary,
                },
            ]
            bump()
        },
        [bump],
    )

    return {
        entries: entries.current,
        version,
        schedule,
        report,
        undo,
        undoLatest,
        dismiss: drop,
        flush,
    }
}
