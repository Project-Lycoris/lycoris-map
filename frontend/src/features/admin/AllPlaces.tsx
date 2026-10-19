import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useSearchParams } from 'react-router'
import { DesignButton } from '@/shared/ui/design-primitives'
import { markerTextSchema, type MarkerText } from '@/shared/api/markerWrites'
import { FacilityTags } from '@/features/places/FacilityTags'
import { VenueTag } from '@/features/places/VenueTag'
import { categoryLabels, openingHours, publicImageUrl } from '@/features/places/model'
import { useAdminUi } from './ui'
import { useAdminWork } from './work'
import * as api from './api'
import { draftFromMarker, MarkerFields } from './MarkerEditor'
import { PlaceMapPreview } from './PlaceMapPreview'
import { ModerationTimeline } from './ModerationTimeline'
import { RejectPanel } from './ReviewDetail'
import { Pagination } from './Pagination'
import { markerItem, type ReviewItem } from './reviewModel'
import { refreshModeration } from './refresh'
import { moderationError, useUndoQueue } from './undoQueue'
import { UndoNotifications } from './UndoNotifications'
import { fullTime, relativeTime } from './time'
import { localizedReason } from './reason'

const categories = ['accessible_toilet', 'baby_room', 'friendly_clinic', 'self_definition'] as const
const statusOptions = [
    ['', 'All statuses'],
    ['PENDING', 'Pending'],
    ['APPROVED', 'Approved'],
    ['REJECTED', 'Rejected'],
] as const
const deactivatedOptions = [
    ['', 'All places'],
    ['false', 'Active only'],
    ['true', 'Disabled only'],
] as const
const sortOptions = [
    ['updated_desc', 'Recently updated'],
    ['created_desc', 'Newest created'],
    ['created_asc', 'Oldest created'],
] as const

export function AllPlaces() {
    const ui = useAdminUi(),
        work = useAdminWork(),
        client = useQueryClient(),
        [params, setParams] = useSearchParams()
    const [search, setSearch] = useState(() => params.get('q') ?? params.get('markerId') ?? '')
    const [removed, setRemoved] = useState<Set<number>>(() => new Set())
    const [rejecting, setRejecting] = useState(false)
    const [editing, setEditing] = useState(false)
    const [toggling, setToggling] = useState(false)

    const q = (params.get('q') ?? '').trim()
    const page = Math.max(0, Number(params.get('page') ?? '0') || 0)
    const status = (params.get('status') ?? '') as '' | 'PENDING' | 'APPROVED' | 'REJECTED'
    const category = params.get('category') ?? ''
    const deactivated = params.get('deactivated') ?? ''
    const sort = (params.get('sort') ?? 'updated_desc') as
        'updated_desc' | 'created_desc' | 'created_asc'

    const setParam = useCallback(
        (changes: Record<string, string>) => {
            const next = new URLSearchParams(params)
            for (const [key, value] of Object.entries(changes)) {
                if (value) next.set(key, value)
                else next.delete(key)
            }
            setParams(next, { replace: true })
        },
        [params, setParams],
    )

    useEffect(() => {
        const trimmed = search.trim()
        if ((params.get('q') ?? '') === trimmed) return
        const timer = window.setTimeout(() => setParam({ q: trimmed, page: '' }), 300)
        return () => window.clearTimeout(timer)
    }, [search, params, setParam])

    // Audit-log links carry an id. Resolve that initial filter before fetching,
    // so an unfiltered response cannot briefly replace the requested detail.
    const jumped = useRef(false)
    useEffect(() => {
        if (jumped.current) return
        jumped.current = true
        const markerId = params.get('markerId')
        if (markerId && !params.get('q')) setParam({ q: markerId })
    }, [params, setParam])

    const query = useQuery({
        enabled: jumped.current || !params.get('markerId') || params.has('q'),
        queryKey: [
            ...work.prefix,
            'all-search',
            { q, page, status, category, deactivated, sort },
            ui.language,
        ],
        gcTime: 0,
        retry: false,
        queryFn: ({ signal }) =>
            work.run(
                (s) =>
                    api.searchMarkers(
                        {
                            page,
                            size: 20,
                            q: q || undefined,
                            status: status || undefined,
                            category: category || undefined,
                            deactivated: deactivated === '' ? undefined : deactivated === 'true',
                            sort,
                            language: ui.language,
                        },
                        s,
                    ),
                signal,
            ),
    })
    const missing = query.isError && api.isMissingRoute(query.error)
    const items = useMemo(() => query.data?.items ?? [], [query.data])
    const visible = useMemo(() => items.filter((item) => !removed.has(item.id)), [items, removed])
    const selectedId = Number(params.get('markerId') ?? '') || null
    const selected = visible.find((item) => item.id === selectedId) ?? null
    const totalPages = Math.max(1, query.data?.totalPages ?? 1)

    const restore = useCallback((list: ReviewItem[]) => {
        setRemoved((previous) => {
            const next = new Set(previous)
            for (const item of list) next.delete(item.id)
            return next
        })
    }, [])
    const commit = useCallback(
        async (list: ReviewItem[] = []) => {
            await refreshModeration(client, work.prefix, work.session.scope)
            // All-status results still contain the reviewed place with its new status.
            restore(list)
        },
        [client, work.prefix, work.session.scope, restore],
    )
    const undo = useUndoQueue({
        run: work.run,
        language: 'zh',
        onRestore: restore,
        onCommit: commit,
    })

    const toggleActive = async (marker: api.AdminMarker) => {
        if (toggling) return
        setToggling(true)
        try {
            await work.run((signal) =>
                marker.deactivated
                    ? api.restoreMarker(marker.id, signal)
                    : api.deactivateMarker(marker.id, signal),
            )
        } finally {
            setToggling(false)
            void commit()
        }
    }

    const decide = (marker: api.AdminMarker, decision: 'approve' | 'reject', reason?: string) => {
        setRemoved((previous) => new Set([...previous, marker.id]))
        undo.schedule({ items: [markerItem(marker)], decision, reason })
        setRejecting(false)
    }

    useEffect(() => {
        setEditing(false)
        setRejecting(false)
    }, [selectedId])

    return (
        <section className="admin-content all-places">
            <div className="admin-toolbar all-places-filters">
                <label className="admin-field all-search">
                    {ui.message('Search')}
                    <input
                        type="search"
                        value={search}
                        maxLength={255}
                        placeholder={ui.message('Search title, submitter or place ID')}
                        onChange={(event) => setSearch(event.target.value)}
                    />
                </label>
                <label className="admin-field">
                    {ui.message('Review status')}
                    <select
                        value={status}
                        onChange={(e) => setParam({ status: e.target.value, page: '' })}
                    >
                        {statusOptions.map(([value, label]) => (
                            <option key={value} value={value}>
                                {ui.message(label)}
                            </option>
                        ))}
                    </select>
                </label>
                <label className="admin-field">
                    {ui.message('Category')}
                    <select
                        value={category}
                        onChange={(e) => setParam({ category: e.target.value, page: '' })}
                    >
                        <option value="">{ui.message('All categories')}</option>
                        {categories.map((value) => (
                            <option key={value} value={value}>
                                {ui.message(categoryLabels[value])}
                            </option>
                        ))}
                    </select>
                </label>
                <label className="admin-field">
                    {ui.message('Disable state')}
                    <select
                        value={deactivated}
                        onChange={(e) => setParam({ deactivated: e.target.value, page: '' })}
                    >
                        {deactivatedOptions.map(([value, label]) => (
                            <option key={value} value={value}>
                                {ui.message(label)}
                            </option>
                        ))}
                    </select>
                </label>
                <label className="admin-field">
                    {ui.message('Sort')}
                    <select
                        value={sort}
                        onChange={(e) => setParam({ sort: e.target.value, page: '' })}
                    >
                        {sortOptions.map(([value, label]) => (
                            <option key={value} value={value}>
                                {ui.message(label)}
                            </option>
                        ))}
                    </select>
                </label>
                <DesignButton disabled={query.isFetching} onClick={() => void query.refetch()}>
                    {ui.message('Refresh')}
                </DesignButton>
            </div>
            <p role="status">
                {ui.message(
                    missing
                        ? 'Server update required to browse all places'
                        : query.isError
                          ? 'The request failed. Refresh and try again.'
                          : query.isFetching
                            ? 'Loading places…'
                            : !visible.length
                              ? 'No items.'
                              : '',
                )}
            </p>
            <div className={`review-master ${selected ? 'has-selection' : ''}`}>
                <div className="review-list-pane">
                    <ul className="review-list" aria-label={ui.message('All places')}>
                        {visible.map((marker) => {
                            const image = publicImageUrl(marker.markImage)
                            return (
                                <li
                                    key={marker.id}
                                    aria-current={selected?.id === marker.id || undefined}
                                >
                                    <button
                                        type="button"
                                        className="review-row"
                                        data-key={marker.id}
                                        aria-current={selected?.id === marker.id || undefined}
                                        onClick={() => setParam({ markerId: String(marker.id) })}
                                    >
                                        {image ? (
                                            <img
                                                className="review-thumb"
                                                src={image}
                                                alt=""
                                                loading="lazy"
                                            />
                                        ) : (
                                            <span className="review-thumb review-thumb-badge">
                                                <span
                                                    className="review-thumb-text"
                                                    aria-hidden="true"
                                                >
                                                    {ui.message(categoryLabels[marker.category])}
                                                </span>
                                            </span>
                                        )}
                                        <span className="review-row-text">
                                            <span className="review-row-title">{marker.title}</span>
                                            <span className="review-row-meta">
                                                {ui.message(categoryLabels[marker.category])}
                                                {' · '}
                                                <MarkerStatus marker={marker} />
                                                {' · '}
                                                <time
                                                    dateTime={marker.updatedAt}
                                                    title={fullTime(marker.updatedAt, ui.language)}
                                                >
                                                    {relativeTime(marker.updatedAt, ui.language)}
                                                </time>
                                            </span>
                                        </span>
                                    </button>
                                </li>
                            )
                        })}
                    </ul>
                    {!visible.length && !query.isPending && !query.isError && (
                        <p className="review-empty">{ui.message('No items.')}</p>
                    )}
                </div>
                <div className="review-detail-pane">
                    <DesignButton
                        className="review-back"
                        onClick={() => setParam({ markerId: '' })}
                    >
                        {ui.message('Back to list')}
                    </DesignButton>
                    {selected ? (
                        <AllPlaceDetail
                            key={selected.id}
                            marker={selected}
                            rejecting={rejecting}
                            editing={editing}
                            busy={work.busy || toggling}
                            onReject={() => setRejecting(true)}
                            onCancelReject={() => setRejecting(false)}
                            onRejectConfirm={(reason) => decide(selected, 'reject', reason)}
                            onApprove={() => decide(selected, 'approve')}
                            onEdit={() => setEditing(true)}
                            onCloseEdit={() => setEditing(false)}
                            onSaved={commit}
                            onToggleActive={toggleActive}
                        />
                    ) : (
                        <aside className="review-detail review-detail-empty" aria-label="Details" />
                    )}
                </div>
            </div>
            <div className="all-places-pager">
                <Pagination
                    page={Math.min(page, totalPages - 1)}
                    total={totalPages}
                    busy={query.isFetching}
                    change={(value) => setParam({ page: value <= 0 ? '' : String(value) })}
                />
                <p>
                    {Math.min(page, totalPages - 1) + 1} / {totalPages} ·{' '}
                    {query.data?.totalElements ?? 0} {ui.message('places')}
                </p>
            </div>
            <UndoNotifications entries={undo.entries} undo={undo.undo} dismiss={undo.dismiss} />
        </section>
    )
}

function MarkerStatus({ marker }: { marker: api.AdminMarker }) {
    const ui = useAdminUi()
    if (marker.deactivated)
        return <span className="marker-status status-disabled">{ui.message('Disabled')}</span>
    const label =
        marker.reviewStatus === 'PENDING'
            ? 'Pending'
            : marker.reviewStatus === 'APPROVED'
              ? 'Approved'
              : 'Rejected'
    return (
        <span className={`marker-status status-${marker.reviewStatus.toLowerCase()}`}>
            {ui.message(label)}
        </span>
    )
}

function AllPlaceDetail({
    marker,
    rejecting,
    editing,
    busy,
    onReject,
    onCancelReject,
    onRejectConfirm,
    onApprove,
    onEdit,
    onCloseEdit,
    onSaved,
    onToggleActive,
}: {
    marker: api.AdminMarker
    rejecting: boolean
    editing: boolean
    busy: boolean
    onReject: () => void
    onCancelReject: () => void
    onRejectConfirm: (reason: string) => void
    onApprove: () => void
    onEdit: () => void
    onCloseEdit: () => void
    onSaved: () => void
    onToggleActive: (marker: api.AdminMarker) => Promise<unknown>
}) {
    const ui = useAdminUi()
    const [confirming, setConfirming] = useState(false)
    const [error, setError] = useState('')
    const image = publicImageUrl(marker.markImage)
    return (
        <aside className="review-detail" aria-label={ui.message('Details')}>
            <header className="review-detail-header">
                <p className="review-kind">
                    <MarkerStatus marker={marker} />
                </p>
                <h2>{marker.title}</h2>
                <p className="admin-meta">
                    #{marker.id} · {ui.message('Submitted by')} {marker.username}
                </p>
                <p className="admin-meta">
                    <time
                        dateTime={marker.updatedAt}
                        title={fullTime(marker.updatedAt, ui.language)}
                    >
                        {relativeTime(marker.updatedAt, ui.language)}
                    </time>
                </p>
            </header>
            <div className="review-detail-body">
                {marker.reviewNote && (
                    <p className="review-note">
                        {ui.message('Rejection reason')}:{' '}
                        {localizedReason(marker.reviewNote, ui.message)}
                    </p>
                )}
                <div className="review-tags">
                    <FacilityTags place={marker} />
                    <VenueTag place={marker} />
                    <span>{ui.message(categoryLabels[marker.category])}</span>
                    <span>{ui.message(marker.isPublic ? 'Public' : 'Private')}</span>
                </div>
                {marker.description && <p className="admin-description">{marker.description}</p>}
                <dl className="review-facts">
                    <div>
                        <dt>{ui.message('Opening hours')}</dt>
                        <dd>{openingHours(marker, ui.language)}</dd>
                    </div>
                    <div>
                        <dt>{ui.message('Opening hours note')}</dt>
                        <dd>{marker.openingHoursNote || '—'}</dd>
                    </div>
                    <div>
                        <dt>{ui.message('Coordinates')}</dt>
                        <dd>
                            {marker.lat}, {marker.lng}
                        </dd>
                    </div>
                </dl>
                <PlaceMapPreview lat={marker.lat} lng={marker.lng} title={marker.title} />
                <a href={`/maps?markerId=${marker.id}`} target="_blank" rel="noreferrer">
                    {ui.message('Open in map')}
                </a>
                {image && (
                    <a href={image} target="_blank" rel="noreferrer">
                        <img className="admin-photo" src={image} alt={ui.message('Photo')} />
                    </a>
                )}
                <ModerationTimeline markerId={marker.id} />
            </div>
            <footer className="review-detail-actions">
                {marker.reviewStatus === 'PENDING' && !marker.deactivated && (
                    <>
                        <DesignButton onClick={onApprove}>{ui.message('Approve')}</DesignButton>
                        <DesignButton onClick={onReject}>{ui.message('Reject')}</DesignButton>
                    </>
                )}
                <DesignButton onClick={onEdit} disabled={!!marker.deactivated}>
                    {ui.message('Edit')}
                </DesignButton>
                <DesignButton
                    disabled={busy || marker.deactivated === undefined}
                    onClick={() => setConfirming(true)}
                >
                    {ui.message(
                        marker.deactivated === undefined
                            ? 'Server update required'
                            : marker.deactivated
                              ? 'Restore'
                              : 'Disable',
                    )}
                </DesignButton>
            </footer>
            {rejecting && (
                <RejectPanel
                    item={markerItem(marker)}
                    close={onCancelReject}
                    submit={onRejectConfirm}
                />
            )}
            {editing && <AllPlaceEditor marker={marker} close={onCloseEdit} saved={onSaved} />}
            {confirming && (
                <section
                    className="review-reject"
                    role="alertdialog"
                    aria-modal="false"
                    aria-labelledby="disable-title"
                >
                    <h3 id="disable-title">
                        {ui.message(marker.deactivated ? 'Restore' : 'Disable')}
                    </h3>
                    <p>
                        {ui.message(
                            marker.deactivated
                                ? 'Restore this place with its previous visibility and review status?'
                                : 'This hides the place from the map. All data is kept and can be restored.',
                        )}
                    </p>
                    <div className="admin-actions">
                        <DesignButton
                            disabled={busy}
                            onClick={() => {
                                setConfirming(false)
                                onToggleActive(marker).catch((reason: unknown) =>
                                    setError(moderationError(reason)),
                                )
                            }}
                        >
                            {ui.message('Confirm')}
                        </DesignButton>
                        <DesignButton onClick={() => setConfirming(false)}>
                            {ui.message('Cancel')}
                        </DesignButton>
                    </div>
                </section>
            )}
            {error && <p role="alert">{ui.message(error)}</p>}
        </aside>
    )
}

export function AllPlaceEditor({
    marker,
    close,
    saved,
}: {
    marker: api.AdminMarker
    close: () => void
    saved?: () => void
}) {
    const ui = useAdminUi(),
        work = useAdminWork(),
        [draft, setDraft] = useState<MarkerText>(() => draftFromMarker(marker)),
        [pending, setPending] = useState<MarkerText | null>(null),
        [error, setError] = useState(''),
        [saving, setSaving] = useState(false)
    return (
        <section className="review-editor" aria-label={ui.message('Edit approved content')}>
            <h3>
                {ui.message('Edit approved content')} · #{marker.id}
            </h3>
            <form
                onSubmit={(event) => {
                    event.preventDefault()
                    const result = markerTextSchema.safeParse(draft)
                    if (!result.success) {
                        setError('Check the title and both opening times.')
                        return
                    }
                    setError('')
                    setPending(result.data)
                }}
            >
                <MarkerFields draft={draft} setDraft={setDraft} disabled={saving} />
                <div className="admin-actions">
                    <DesignButton type="submit">{ui.message('Save')}</DesignButton>
                    <DesignButton onClick={close}>{ui.message('Cancel')}</DesignButton>
                </div>
                {error && <p role="alert">{ui.message(error)}</p>}
            </form>
            {pending && (
                <section
                    className="review-inline-confirm"
                    role="alertdialog"
                    aria-modal="false"
                    aria-labelledby="edit-confirm-title"
                >
                    <h4 id="edit-confirm-title">
                        {ui.message('Edit approved content')} · {pending.title}
                    </h4>
                    <p>{ui.message('This saves the changes as approved content.')}</p>
                    <div className="admin-actions">
                        <DesignButton
                            disabled={saving}
                            onClick={() => {
                                setSaving(true)
                                work.run((signal) => api.editMarker(marker.id, pending, signal))
                                    .then(() => {
                                        setPending(null)
                                        saved?.()
                                        close()
                                    })
                                    .catch((reason: unknown) => setError(moderationError(reason)))
                                    .finally(() => setSaving(false))
                            }}
                        >
                            {ui.message('Confirm')}
                        </DesignButton>
                        <DesignButton onClick={() => setPending(null)}>
                            {ui.message('Cancel')}
                        </DesignButton>
                    </div>
                </section>
            )}
        </section>
    )
}
