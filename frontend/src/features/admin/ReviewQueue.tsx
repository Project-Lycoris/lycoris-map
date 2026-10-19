import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useSearchParams } from 'react-router'
import { CategoryBadge, DesignButton } from '@/shared/ui/design-primitives'
import { categoryBadges, categoryLabels, publicImageUrl } from '@/features/places/model'
import type { MarkerText } from '@/shared/api/markerWrites'
import { useAdminUi } from './ui'
import { useAdminWork } from './work'
import * as api from './api'
import { ReviewDetail } from './ReviewDetail'
import { fullTime, relativeTime } from './time'
import {
    editItem,
    imageItem,
    itemMatches,
    kindLabel,
    markerItem,
    type ReviewItem,
    type ReviewKind,
} from './reviewModel'
import { useUndoQueue } from './undoQueue'
import { UndoNotifications } from './UndoNotifications'

const quickBatchReasons = [
    'Information is inaccurate',
    'Duplicate place',
    'Wrong location',
    'Inappropriate photo',
    'Unrelated to accessibility or friendly facilities',
]

type SortOrder = 'oldest' | 'newest'
type Filter = 'all' | ReviewKind

export function ReviewQueue() {
    const ui = useAdminUi(),
        work = useAdminWork(),
        client = useQueryClient(),
        [params, setParams] = useSearchParams(),
        [search, setSearch] = useState(''),
        [filter, setFilter] = useState<Filter>('all'),
        [sort, setSort] = useState<SortOrder>('oldest'),
        [selected, setSelected] = useState<string | null>(() => params.get('item')),
        [removed, setRemoved] = useState<Set<string>>(() => new Set()),
        [checked, setChecked] = useState<Set<string>>(() => new Set()),
        [batchReject, setBatchReject] = useState(false),
        [batchReason, setBatchReason] = useState(''),
        [rejectRequest, setRejectRequest] = useState(0),
        [editRequest, setEditRequest] = useState(0),
        [overlayOpen, setOverlayOpen] = useState(false),
        [help, setHelp] = useState(false)

    const query = useQuery({
        queryKey: [...work.prefix, 'review', ui.language],
        gcTime: 0,
        retry: false,
        queryFn: ({ signal }): Promise<ReviewItem[]> =>
            work.run(async (s) => {
                const [markers, edits, images] = await Promise.all([
                    api.readPendingMarkers(s),
                    api.readEdits(s),
                    api.readImages(s),
                ])
                return [
                    ...markers.map(markerItem),
                    ...edits.map(editItem),
                    ...images.map(imageItem),
                ]
            }, signal),
    })

    const all = useMemo(() => query.data ?? [], [query.data])
    const queue = useMemo(() => all.filter((item) => !removed.has(item.key)), [all, removed])
    const counts = useMemo(() => {
        const searched = queue.filter((item) => itemMatches(item, search))
        return {
            all: searched.length,
            marker: searched.filter((item) => item.kind === 'marker').length,
            edit: searched.filter((item) => item.kind === 'edit').length,
            image: searched.filter((item) => item.kind === 'image').length,
        }
    }, [queue, search])
    const filtered = useMemo(() => {
        const searched = queue.filter(
            (item) => itemMatches(item, search) && (filter === 'all' || item.kind === filter),
        )
        searched.sort((a, b) => a.createdAt.localeCompare(b.createdAt) || a.id - b.id)
        return sort === 'oldest' ? searched : searched.reverse()
    }, [queue, search, filter, sort])

    const restore = useCallback((items: ReviewItem[]) => {
        setRemoved((previous) => {
            const next = new Set(previous)
            for (const item of items) next.delete(item.key)
            return next
        })
        setSelected((previous) => items[0]?.key ?? previous)
    }, [])
    const commit = useCallback(
        (items: ReviewItem[]) => {
            setChecked((previous) => {
                if (!items.some((item) => previous.has(item.key))) return previous
                const next = new Set(previous)
                for (const item of items) next.delete(item.key)
                return next
            })
            void client.invalidateQueries({
                queryKey: [...work.prefix, 'current'],
                refetchType: 'none',
            })
        },
        [client, work.prefix],
    )
    const undo = useUndoQueue({
        run: work.run,
        // Pending submissions are authored in Chinese; keep the existing
        // moderation `lang` semantics independent of the dashboard locale.
        language: 'zh',
        onRestore: restore,
        onCommit: commit,
    })

    useEffect(() => {
        if (selected && filtered.some((item) => item.key === selected)) return
        const fromUrl = params.get('item')
        const fromQuery = filtered.find((item) => item.key === fromUrl)
        // Narrow screens start on the list so the first item is not opened
        // before the reviewer chooses one; desktop master-detail selects it.
        const wide =
            typeof window.matchMedia === 'function'
                ? window.matchMedia('(min-width: 1024px)').matches
                : window.innerWidth >= 1024
        const target = fromQuery ?? (wide ? filtered[0] : undefined)
        setSelected(target?.key ?? null)
    }, [filtered, selected, params])

    useEffect(() => {
        if (!selected) return
        if (params.get('item') === selected) return
        const next = new URLSearchParams(params)
        next.set('item', selected)
        setParams(next, { replace: true })
    }, [selected, params, setParams])

    useEffect(() => {
        setChecked((previous) => {
            const keys = new Set(queue.map((item) => item.key))
            const next = new Set([...previous].filter((key) => keys.has(key)))
            return next.size === previous.size ? previous : next
        })
    }, [queue])

    const handleOverlayChange = useCallback((open: boolean) => setOverlayOpen(open), [])

    const selectedItem = filtered.find((item) => item.key === selected) ?? null

    const removeAndSelectNext = (items: ReviewItem[]) => {
        const keys = new Set(items.map((item) => item.key))
        const index = filtered.findIndex((item) => item.key === selected)
        const remaining = filtered.filter((item) => !keys.has(item.key))
        const next = remaining[Math.min(Math.max(index, 0), Math.max(remaining.length - 1, 0))]
        setRemoved((previous) => new Set([...previous, ...keys]))
        setChecked((previous) => {
            if (!items.some((item) => previous.has(item.key))) return previous
            const nextChecked = new Set(previous)
            for (const key of keys) nextChecked.delete(key)
            return nextChecked
        })
        setSelected(next?.key ?? null)
    }

    const approve = (item: ReviewItem) => {
        removeAndSelectNext([item])
        undo.schedule({ items: [item], decision: 'approve' })
    }
    const reject = (item: ReviewItem, reason: string) => {
        removeAndSelectNext([item])
        undo.schedule({ items: [item], decision: 'reject', reason })
    }
    const approveWithChanges = async (item: ReviewItem, changes: MarkerText) => {
        await work.run((signal) =>
            api.moderate(
                item.kind === 'marker' ? 'markers' : 'edits',
                item.id,
                'approve',
                'zh',
                signal,
                { changes },
            ),
        )
        removeAndSelectNext([item])
        commit([item])
    }
    const selectedItems = filtered.filter((item) => checked.has(item.key))
    const bulk = (decision: 'approve' | 'reject', reason?: string) => {
        if (!selectedItems.length) return
        removeAndSelectNext(selectedItems)
        undo.schedule({ items: selectedItems, decision, reason })
        setBatchReject(false)
        setBatchReason('')
    }

    const toggleChecked = (key: string) =>
        setChecked((previous) => {
            const next = new Set(previous)
            if (next.has(key)) next.delete(key)
            else next.add(key)
            return next
        })
    const select = (key: string) => setSelected(key)
    const allFilteredChecked =
        filtered.length > 0 && filtered.every((item) => checked.has(item.key))

    useEffect(() => {
        const keydown = (event: KeyboardEvent) => {
            if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey) return
            if (event.isComposing) return
            const target = event.target
            if (
                target instanceof HTMLElement &&
                (target.matches('input, textarea, select, [contenteditable="true"]') ||
                    target.closest('[contenteditable="true"]'))
            )
                return
            const key = event.key
            // A reason form or in-place editor owns the keyboard; only Escape
            // (handled by that panel) may act while it is open.
            if (overlayOpen) return
            if (key === '?' || (key === '/' && event.shiftKey)) {
                event.preventDefault()
                setHelp((value) => !value)
                return
            }
            if (key === 'Escape') {
                setHelp(false)
                return
            }
            if (key === 'j' || key === 'J' || key === 'ArrowDown') {
                event.preventDefault()
                const index = filtered.findIndex((item) => item.key === selected)
                const next = filtered[Math.min(index + 1, filtered.length - 1)]
                if (next) select(next.key)
                return
            }
            if (key === 'k' || key === 'K' || key === 'ArrowUp') {
                event.preventDefault()
                const index = filtered.findIndex((item) => item.key === selected)
                const next = filtered[Math.max(index - 1, 0)]
                if (next) select(next.key)
                return
            }
            if (!selectedItem) return
            if (key === 'a' || key === 'A') {
                if (
                    selectedItem.kind === 'image' ||
                    selectedItem.kind === 'marker' ||
                    selectedItem.kind === 'edit'
                ) {
                    event.preventDefault()
                    approve(selectedItem)
                }
                return
            }
            if (key === 'r' || key === 'R') {
                event.preventDefault()
                setRejectRequest((value) => value + 1)
                return
            }
            if (key === 'e' || key === 'E') {
                if (selectedItem.kind === 'image') return
                event.preventDefault()
                setEditRequest((value) => value + 1)
                return
            }
            if (key === 'x' || key === 'X') {
                event.preventDefault()
                toggleChecked(selectedItem.key)
                return
            }
            if (key === 'z' || key === 'Z') {
                event.preventDefault()
                undo.undoLatest()
            }
        }
        window.addEventListener('keydown', keydown)
        return () => window.removeEventListener('keydown', keydown)
    })

    const listRef = useRef<HTMLUListElement>(null)

    useEffect(() => {
        if (!selected || !listRef.current) return
        const row = listRef.current.querySelector<HTMLElement>(
            `[data-key="${CSS.escape(selected)}"]`,
        )
        row?.scrollIntoView?.({ block: 'nearest' })
    }, [selected])

    return (
        <section className="admin-content review-queue">
            <div className="review-toolbar">
                <div className="review-tabs" role="group" aria-label={ui.message('Filter by type')}>
                    {(
                        [
                            ['all', 'All', counts.all],
                            ['marker', 'New place', counts.marker],
                            ['edit', 'Content changes', counts.edit],
                            ['image', 'Photo submission', counts.image],
                        ] as const
                    ).map(([value, label, count]) => (
                        <button
                            key={value}
                            type="button"
                            aria-pressed={filter === value}
                            onClick={() => setFilter(value)}
                        >
                            {ui.message(label)} ({count})
                        </button>
                    ))}
                </div>
                <div className="review-tools">
                    <label className="admin-field review-search">
                        {ui.message('Search')}
                        <input
                            type="search"
                            value={search}
                            maxLength={255}
                            placeholder={ui.message('Search title, submitter or place ID')}
                            onChange={(event) => setSearch(event.target.value)}
                        />
                    </label>
                    <label className="admin-field review-sort">
                        {ui.message('Sort')}
                        <select
                            value={sort}
                            onChange={(event) =>
                                setSort(event.target.value === 'newest' ? 'newest' : 'oldest')
                            }
                        >
                            <option value="oldest">{ui.message('Oldest first')}</option>
                            <option value="newest">{ui.message('Newest first')}</option>
                        </select>
                    </label>
                    <DesignButton disabled={query.isFetching} onClick={() => void query.refetch()}>
                        {ui.message('Refresh')}
                    </DesignButton>
                </div>
            </div>
            <p role="status">
                {ui.message(
                    query.isError
                        ? 'The request failed. Refresh and try again.'
                        : query.isFetching
                          ? 'Loading places…'
                          : !queue.length
                            ? 'No items.'
                            : '',
                )}
            </p>
            {checked.size > 0 && (
                <div className="review-bulk" role="region" aria-label={ui.message('Bulk actions')}>
                    <strong>
                        {checked.size} {ui.message('selected')}
                    </strong>
                    <DesignButton onClick={() => bulk('approve')}>
                        {ui.message('Approve')}
                    </DesignButton>
                    <DesignButton onClick={() => setBatchReject((value) => !value)}>
                        {ui.message('Reject')}
                    </DesignButton>
                    <DesignButton onClick={() => setChecked(new Set())}>
                        {ui.message('Clear selection')}
                    </DesignButton>
                    <label className="review-select-all">
                        <input
                            type="checkbox"
                            checked={allFilteredChecked}
                            onChange={() =>
                                setChecked(
                                    allFilteredChecked
                                        ? new Set()
                                        : new Set(filtered.map((item) => item.key)),
                                )
                            }
                        />
                        {ui.message('Select all filtered')} ({filtered.length})
                    </label>
                    {batchReject && (
                        <div className="review-bulk-reject">
                            <label className="admin-field">
                                {ui.message('Shared reason (optional)')}
                                <input
                                    value={batchReason}
                                    maxLength={500}
                                    onChange={(event) => setBatchReason(event.target.value)}
                                />
                            </label>
                            <DesignButton onClick={() => bulk('reject', batchReason)}>
                                {ui.message('Confirm reject')}
                            </DesignButton>
                            <DesignButton onClick={() => setBatchReject(false)}>
                                {ui.message('Cancel')}
                            </DesignButton>
                            <div className="admin-actions">
                                {quickBatchReasons
                                    .filter(
                                        (value) =>
                                            value !== 'Inappropriate photo' ||
                                            selectedItems.some((item) => item.kind === 'image'),
                                    )
                                    .map((value) => (
                                        <DesignButton
                                            key={value}
                                            aria-pressed={batchReason === value}
                                            onClick={() =>
                                                setBatchReason(batchReason === value ? '' : value)
                                            }
                                        >
                                            {ui.message(value)}
                                        </DesignButton>
                                    ))}
                            </div>
                        </div>
                    )}
                </div>
            )}
            {checked.size === 0 && (
                <p className="review-select-all">
                    <label>
                        <input
                            type="checkbox"
                            checked={allFilteredChecked}
                            disabled={!filtered.length}
                            onChange={() =>
                                setChecked(
                                    allFilteredChecked
                                        ? new Set()
                                        : new Set(filtered.map((item) => item.key)),
                                )
                            }
                        />
                        {ui.message('Select all filtered')} ({filtered.length})
                    </label>
                </p>
            )}
            <div className={`review-master ${selectedItem ? 'has-selection' : ''}`}>
                <div className="review-list-pane">
                    <ul
                        className="review-list"
                        ref={listRef}
                        aria-label={ui.message('Review queue')}
                    >
                        {filtered.map((item) => {
                            const title = item.kind === 'marker' ? item.marker.title : item.title
                            const image =
                                item.kind === 'image'
                                    ? publicImageUrl(item.image.imageUrl)
                                    : item.kind === 'marker'
                                      ? publicImageUrl(item.marker.markImage)
                                      : null
                            const category =
                                item.kind === 'marker'
                                    ? item.marker.category
                                    : item.kind === 'edit'
                                      ? item.edit.category
                                      : null
                            const badge = category ? categoryBadges[category] : null
                            return (
                                <li
                                    key={item.key}
                                    aria-current={selected === item.key || undefined}
                                >
                                    <input
                                        type="checkbox"
                                        aria-label={`${ui.message('Select')} ${title}`}
                                        checked={checked.has(item.key)}
                                        onChange={() => toggleChecked(item.key)}
                                    />
                                    <button
                                        type="button"
                                        className="review-row"
                                        data-key={item.key}
                                        aria-current={selected === item.key || undefined}
                                        onClick={() => select(item.key)}
                                    >
                                        {image ? (
                                            <img
                                                className="review-thumb"
                                                src={image}
                                                alt=""
                                                loading="lazy"
                                            />
                                        ) : badge ? (
                                            <span className="review-thumb review-thumb-badge">
                                                <CategoryBadge category={badge} />
                                            </span>
                                        ) : category ? (
                                            <span className="review-thumb review-thumb-badge">
                                                <span
                                                    className="review-thumb-text"
                                                    aria-hidden="true"
                                                >
                                                    {ui.message(categoryLabels[category])}
                                                </span>
                                            </span>
                                        ) : (
                                            <span
                                                className="review-thumb review-thumb-empty"
                                                aria-hidden="true"
                                            />
                                        )}
                                        <span className="review-row-text">
                                            <span className="review-kind">
                                                {ui.message(kindLabel(item.kind))}
                                            </span>
                                            <span className="review-row-title">{title}</span>
                                            <span className="review-row-meta">
                                                {item.submitter} ·{' '}
                                                <time
                                                    dateTime={item.createdAt}
                                                    title={fullTime(item.createdAt, ui.language)}
                                                >
                                                    {relativeTime(item.createdAt, ui.language)}
                                                </time>
                                            </span>
                                        </span>
                                    </button>
                                </li>
                            )
                        })}
                    </ul>
                    {!filtered.length && !query.isPending && !query.isError && (
                        <p className="review-empty">{ui.message('No matching items.')}</p>
                    )}
                </div>
                <div className="review-detail-pane">
                    <DesignButton
                        className="review-back"
                        onClick={() => {
                            setSelected(null)
                            const next = new URLSearchParams(params)
                            next.delete('item')
                            setParams(next, { replace: true })
                        }}
                    >
                        {ui.message('Back to list')}
                    </DesignButton>
                    <ReviewDetail
                        item={selectedItem}
                        onApprove={approve}
                        onReject={reject}
                        onApproveWithChanges={approveWithChanges}
                        onOverlayChange={handleOverlayChange}
                        rejectRequest={rejectRequest}
                        editRequest={editRequest}
                    />
                </div>
            </div>
            <UndoNotifications entries={undo.entries} undo={undo.undo} dismiss={undo.dismiss} />
            {help && <ShortcutHelp close={() => setHelp(false)} />}
        </section>
    )
}

function ShortcutHelp({ close }: { close: () => void }) {
    const ui = useAdminUi()
    const shortcuts: [string, string][] = [
        ['J / ↓', 'Next item'],
        ['K / ↑', 'Previous item'],
        ['A', 'Approve'],
        ['R', 'Reject'],
        ['E', 'Modify and approve'],
        ['X', 'Select item'],
        ['Z', 'Undo last action'],
        ['?', 'Keyboard shortcuts'],
    ]
    return (
        <div
            className="review-shortcuts"
            role="dialog"
            aria-label={ui.message('Keyboard shortcuts')}
        >
            <h2>{ui.message('Keyboard shortcuts')}</h2>
            <dl>
                {shortcuts.map(([key, label]) => (
                    <div key={key}>
                        <dt>
                            <kbd>{key}</kbd>
                        </dt>
                        <dd>{ui.message(label)}</dd>
                    </div>
                ))}
            </dl>
            <DesignButton onClick={close}>{ui.message('Close')}</DesignButton>
        </div>
    )
}
