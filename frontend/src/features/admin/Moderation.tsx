import { VenueTag } from '@/features/places/VenueTag'
import { useEffect, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import type { Marker } from '@/shared/api/markers'
import { DesignButton, CategoryBadge } from '@/shared/ui/design-primitives'
import {
    categoryBadges,
    categoryLabels,
    openingHours,
    publicImageUrl,
} from '@/features/places/model'
import * as api from './api'
import { useAdminWork } from './work'
import { useAdminUi } from './ui'
import { MarkerEditor } from './MarkerEditor'
import { readAccountPlace } from '@/shared/api/privatePlaces'
type Entry =
    | { kind: 'markers'; item: Marker }
    | { kind: 'edits'; item: api.EditProposal }
    | { kind: 'images'; item: api.ImageProposal }
export function Moderation({ all }: { all: boolean }) {
    const work = useAdminWork(),
        ui = useAdminUi(),
        [page, setPage] = useState(0),
        [editing, setEditing] = useState<Marker | null>(null)
    const query = useQuery({
        queryKey: [...work.prefix, all ? 'all' : 'review', ui.language],
        gcTime: 0,
        retry: false,
        queryFn: ({ signal }): Promise<Entry[]> =>
            work.run(async (s) => {
                const markers = await api.readMarkers(all, all ? ui.language : 'zh', s)
                if (all) return markers.map((item) => ({ kind: 'markers', item }) as const)
                const [edits, images] = await Promise.all([api.readEdits(s), api.readImages(s)])
                return [
                    ...markers.map((item) => ({ kind: 'markers', item }) as const),
                    ...edits.map((item) => ({ kind: 'edits', item }) as const),
                    ...images.map((item) => ({ kind: 'images', item }) as const),
                ]
            }, signal),
    })
    // Paginate places rather than submissions, so a place's photos never land
    // on another page from its content. Oldest pending places are reviewed first.
    const groups = new Map<number, Entry[]>()
    const entries = query.data ?? []
    const ordered = all
        ? entries
        : [...entries].sort(
              (a, b) => a.item.createdAt.localeCompare(b.item.createdAt) || a.item.id - b.item.id,
          )
    for (const entry of ordered) {
        const id = entry.kind === 'markers' ? entry.item.id : entry.item.markerId
        const group = groups.get(id) ?? []
        group.push(entry)
        groups.set(id, group)
    }
    const items = [...groups],
        total = Math.max(1, Math.ceil(items.length / 20)),
        current = Math.min(page, total - 1)
    useEffect(() => {
        setPage(0)
        setEditing(null)
        work.clearConfirmation()
    }, [all, ui.language])
    if (editing) return <MarkerEditor marker={editing} close={() => setEditing(null)} />
    return (
        <section className="admin-content">
            <div className="admin-toolbar">
                <DesignButton
                    disabled={query.isFetching || work.busy}
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
                          ? 'Loading places…'
                          : !items.length
                            ? 'No items.'
                            : '',
                )}
            </p>
            {!query.isError && !query.isPending && (
                <div className="admin-list">
                    {items.slice(current * 20, (current + 1) * 20).map(([id, entries]) => (
                        <section className="admin-place-group" key={id} aria-label={`#${id}`}>
                            {entries.map((entry) => (
                                <ReviewCard
                                    key={`${entry.kind}:${entry.item.id}`}
                                    entry={entry}
                                    all={all}
                                    edit={setEditing}
                                />
                            ))}
                        </section>
                    ))}
                </div>
            )}
            <Pagination
                page={current}
                total={total}
                busy={work.busy || query.isFetching}
                change={setPage}
            />
        </section>
    )
}
function ReviewCard({
    entry,
    all,
    edit,
}: {
    entry: Entry
    all: boolean
    edit: (marker: Marker) => void
}) {
    const ui = useAdminUi(),
        work = useAdminWork(),
        { item } = entry,
        client = useQueryClient()
    const title = entry.kind === 'markers' ? entry.item.title : entry.item.markerTitle
    const proposalLanguage = 'zh'
    const current = useQuery({
        queryKey: [
            ...work.prefix,
            'current',
            entry.kind === 'edits' ? entry.item.markerId : 0,
            proposalLanguage,
        ],
        enabled: entry.kind === 'edits',
        retry: false,
        gcTime: 0,
        queryFn: ({ signal }) =>
            work.run(
                (s) =>
                    readAccountPlace(
                        String(entry.kind === 'edits' ? entry.item.markerId : 0),
                        proposalLanguage,
                        s,
                    ),
                signal,
            ),
    })
    const creator = entry.kind === 'markers' ? entry.item.username : entry.item.proposerUsername
    const image =
        entry.kind === 'images'
            ? publicImageUrl(entry.item.imageUrl)
            : entry.kind === 'markers'
              ? publicImageUrl(entry.item.markImage)
              : null
    const decide = (decision: 'approve' | 'reject') =>
        work.confirm({
            label: `${ui.message(decision === 'approve' ? 'Approve' : 'Reject')} · ${ui.message({ markers: 'New place', edits: 'Content changes', images: 'Photo submission' }[entry.kind])} · ${title} #${item.id}`,
            detail: entry.kind === 'edits' ? entry.item.title : title,
            action: (signal) => api.moderate(entry.kind, item.id, decision, 'zh', signal),
            onSuccess: () => {
                // A photo or content decision only changes this place's baseline.
                const currentKey = [
                    ...work.prefix,
                    'current',
                    entry.kind === 'markers' ? item.id : entry.item.markerId,
                ]
                if (client.getQueriesData({ queryKey: currentKey }).length)
                    void client.invalidateQueries({ queryKey: currentKey }).catch(() => undefined)
                // Remove only the confirmed submission while the queue refreshes.
                client.setQueriesData<Entry[]>(
                    {
                        queryKey: [...work.prefix, 'review'],
                    },
                    (entries) =>
                        entries?.filter(
                            (row) => row.kind !== entry.kind || row.item.id !== item.id,
                        ),
                )
            },
        })
    return (
        <article className="admin-card">
            {!all && (
                <p className="admin-submission-kind">
                    {ui.message(
                        {
                            markers: 'New place',
                            edits: 'Content changes',
                            images: 'Photo submission',
                        }[entry.kind],
                    )}
                </p>
            )}
            <h2>{title}</h2>
            <p className="admin-meta">
                #{entry.kind === 'markers' ? item.id : entry.item.markerId} ·{' '}
                {ui.message('Submitted by')} {creator}
            </p>
            <p className="admin-meta">
                {ui.message('Submitted')}: <time dateTime={item.createdAt}>{item.createdAt}</time>
            </p>
            {entry.kind !== 'images' && (
                <>
                    <VenueTag place={entry.item} />
                    <p className="admin-category">
                        {categoryBadges[entry.item.category] && (
                            <CategoryBadge category={categoryBadges[entry.item.category]!} />
                        )}
                        {ui.message(categoryLabels[entry.item.category])} ·{' '}
                        {ui.message(entry.item.isPublic ? 'Public' : 'Private')}
                    </p>
                    <p className="admin-meta">
                        {ui.message('Location')}: {entry.item.lat}, {entry.item.lng} ·{' '}
                        {openingHours(entry.item, ui.language)}
                    </p>
                    {entry.kind === 'edits' && (
                        <>
                            <h3>{ui.message('Current place')}</h3>
                            {current.data && !current.isError ? (
                                <>
                                    <p>{current.data.title}</p>
                                    <p className="admin-description">
                                        {ui.message('Description')}:{' '}
                                        {current.data.description || '—'}
                                    </p>
                                    <p>
                                        <VenueTag place={current.data} />{' '}
                                        {ui.message(categoryLabels[current.data.category])} ·{' '}
                                        {ui.message(current.data.isPublic ? 'Public' : 'Private')} ·{' '}
                                        {openingHours(current.data, ui.language)}
                                    </p>
                                    <p>
                                        {ui.message('Active flag')}:{' '}
                                        {ui.message(current.data.isActive ? 'Yes' : 'No')}
                                    </p>
                                    {current.data.contentLanguage !== entry.item.language && (
                                        <p>
                                            {ui.message(
                                                'Original content shown because this translation is unavailable.',
                                            )}
                                        </p>
                                    )}
                                </>
                            ) : (
                                <p role="status">
                                    {ui.message(
                                        current.isError
                                            ? 'This place is unavailable.'
                                            : 'Loading places…',
                                    )}
                                    {current.isError && (
                                        <DesignButton onClick={() => void current.refetch()}>
                                            {ui.message('Retry')}
                                        </DesignButton>
                                    )}
                                </p>
                            )}
                            <h3>{ui.message('Proposed changes')}</h3>
                            <p>{entry.item.title}</p>
                            <p>
                                {ui.message(categoryLabels[entry.item.category])} ·{' '}
                                {ui.message(entry.item.isPublic ? 'Public' : 'Private')} ·{' '}
                                {openingHours(entry.item, ui.language)}
                            </p>
                            <p>
                                {ui.message('Language')}: {entry.item.language}
                            </p>
                            <p>
                                {ui.message('Active flag')}:{' '}
                                {ui.message(entry.item.isActive ? 'Yes' : 'No')}
                            </p>
                            <p className="admin-description">
                                {ui.message('Description')}: {entry.item.description || '—'}
                            </p>
                        </>
                    )}
                    {entry.kind === 'markers' && entry.item.description && (
                        <p className="admin-description">{entry.item.description}</p>
                    )}
                    {entry.kind === 'markers' && (
                        <p>
                            {ui.message('Review status')}:{' '}
                            {ui.message(entry.item.reviewStatus.toLowerCase())}
                            {all && (
                                <> · {ui.message(entry.item.deactivated ? 'Disabled' : 'Active')}</>
                            )}
                        </p>
                    )}
                </>
            )}
            {image && <AdminImage key={image} src={image} />}
            {entry.kind === 'images' && !image && <p>{ui.message('Image unavailable.')}</p>}
            <div className="admin-actions">
                <DesignButton
                    disabled={
                        work.busy ||
                        (entry.kind === 'markers' && !!entry.item.deactivated) ||
                        (entry.kind === 'edits' &&
                            (!current.data || current.isError || current.isFetching))
                    }
                    onClick={() => decide('approve')}
                >
                    {ui.message('Approve')}
                </DesignButton>
                <DesignButton
                    disabled={work.busy || (entry.kind === 'markers' && !!entry.item.deactivated)}
                    onClick={() => decide('reject')}
                >
                    {ui.message('Reject')}
                </DesignButton>
                {all && entry.kind === 'markers' && (
                    <>
                        <DesignButton
                            disabled={work.busy || !!entry.item.deactivated}
                            onClick={() => edit(entry.item)}
                        >
                            {ui.message('Edit')}
                        </DesignButton>
                        <DesignButton
                            disabled={work.busy || entry.item.deactivated === undefined}
                            onClick={() =>
                                work.confirm({
                                    label: `${ui.message(entry.item.deactivated ? 'Restore' : 'Disable')} · ${title} #${item.id}`,
                                    detail: ui.message(
                                        entry.item.deactivated
                                            ? 'Restore this place with its previous visibility and review status?'
                                            : 'This hides the place from the map. All data is kept and can be restored.',
                                    ),
                                    action: (signal) =>
                                        entry.item.deactivated
                                            ? api.restoreMarker(item.id, signal)
                                            : api.deactivateMarker(item.id, signal),
                                })
                            }
                        >
                            {ui.message(
                                entry.item.deactivated === undefined
                                    ? 'Server update required'
                                    : entry.item.deactivated
                                      ? 'Restore'
                                      : 'Disable',
                            )}
                        </DesignButton>
                    </>
                )}
            </div>
        </article>
    )
}
function AdminImage({ src }: { src: string }) {
    const [failed, setFailed] = useState(false),
        ui = useAdminUi()
    return failed ? (
        <p>{ui.message('Image unavailable.')}</p>
    ) : (
        <img
            className="admin-photo"
            src={src}
            alt={ui.message('Photo')}
            loading="lazy"
            onError={() => setFailed(true)}
        />
    )
}
export function Pagination({
    page,
    total,
    busy,
    change,
}: {
    page: number
    total: number
    busy: boolean
    change: (page: number) => void
}) {
    const ui = useAdminUi()
    return (
        <nav className="admin-pagination" aria-label={ui.message('Pages')}>
            <DesignButton disabled={busy || page === 0} onClick={() => change(page - 1)}>
                {ui.message('Previous')}
            </DesignButton>
            <span>
                {page + 1} / {total}
            </span>
            <DesignButton disabled={busy || page + 1 >= total} onClick={() => change(page + 1)}>
                {ui.message('Next')}
            </DesignButton>
        </nav>
    )
}
