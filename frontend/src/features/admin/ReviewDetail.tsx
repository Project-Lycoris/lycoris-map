import { useEffect, useRef, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { markerTextSchema, type MarkerText } from '@/shared/api/markerWrites'
import { DesignButton } from '@/shared/ui/design-primitives'
import { readAccountPlace } from '@/shared/api/privatePlaces'
import { FacilityTags } from '@/features/places/FacilityTags'
import { VenueTag } from '@/features/places/VenueTag'
import { categoryLabels, openingHours, publicImageUrl } from '@/features/places/model'
import { useAdminUi } from './ui'
import { useAdminWork } from './work'
import { draftFromMarker, MarkerFields } from './MarkerEditor'
import { PlaceMapPreview } from './PlaceMapPreview'
import { ModerationTimeline } from './ModerationTimeline'
import { fullTime, relativeTime } from './time'
import { comparisonRows } from './comparison'
import { moderationError } from './undoQueue'
import { itemMarkerId, proposalDraft, type ReviewItem } from './reviewModel'

const quickReasons = [
    'Information is inaccurate',
    'Duplicate place',
    'Wrong location',
    'Inappropriate photo',
    'Unrelated to accessibility or friendly facilities',
]

export function ReviewDetail({
    item,
    onApprove,
    onReject,
    onApproveWithChanges,
    onOverlayChange,
    rejectRequest = 0,
    editRequest = 0,
}: {
    item: ReviewItem | null
    onApprove: (item: ReviewItem) => void
    onReject: (item: ReviewItem, reason: string) => void
    onApproveWithChanges: (item: ReviewItem, changes: MarkerText) => Promise<void>
    onOverlayChange?: (open: boolean) => void
    rejectRequest?: number
    editRequest?: number
}) {
    if (!item) return <aside className="review-detail review-detail-empty" aria-label="Details" />
    return (
        <DetailBody
            key={item.key}
            item={item}
            onApprove={onApprove}
            onReject={onReject}
            onApproveWithChanges={onApproveWithChanges}
            onOverlayChange={onOverlayChange}
            rejectRequest={rejectRequest}
            editRequest={editRequest}
        />
    )
}

function DetailBody({
    item,
    onApprove,
    onReject,
    onApproveWithChanges,
    onOverlayChange,
    rejectRequest,
    editRequest,
}: {
    item: ReviewItem
    onApprove: (item: ReviewItem) => void
    onReject: (item: ReviewItem, reason: string) => void
    onApproveWithChanges: (item: ReviewItem, changes: MarkerText) => Promise<void>
    onOverlayChange: ((open: boolean) => void) | undefined
    rejectRequest: number
    editRequest: number
}) {
    const ui = useAdminUi()
    const [rejecting, setRejecting] = useState(false)
    const [editing, setEditing] = useState(false)
    const rejectTrigger = useRef<HTMLButtonElement>(null)
    useEffect(() => {
        if (rejectRequest) setRejecting(true)
    }, [rejectRequest])
    useEffect(() => {
        if (editRequest && item.kind !== 'image') setEditing(true)
    }, [editRequest, item.kind])
    useEffect(() => {
        onOverlayChange?.(rejecting || editing)
        return () => onOverlayChange?.(false)
    }, [rejecting, editing, onOverlayChange])
    const closeReject = () => {
        setRejecting(false)
        requestAnimationFrame(() => rejectTrigger.current?.focus())
    }
    return (
        <aside className="review-detail" aria-label={ui.message('Review details')}>
            <header className="review-detail-header">
                <p className="review-kind">{ui.message(kindLabelFor(item))}</p>
                <h2>{item.title}</h2>
                <p className="admin-meta">
                    #{item.kind === 'marker' ? item.id : item.markerId} ·{' '}
                    {ui.message('Submitted by')} {item.submitter}
                </p>
                <p className="admin-meta">
                    <time dateTime={item.createdAt} title={fullTime(item.createdAt, ui.language)}>
                        {relativeTime(item.createdAt, ui.language)}
                    </time>
                    {' · '}
                    {fullTime(item.createdAt, ui.language)}
                </p>
            </header>
            <div className="review-detail-body">
                {item.kind === 'marker' && <NewPlaceDetail item={item} />}
                {item.kind === 'edit' && <EditDetail item={item} />}
                {item.kind === 'image' && <ImageDetail item={item} />}
                <ModerationTimeline markerId={itemMarkerId(item)} />
            </div>
            <footer className="review-detail-actions">
                <DesignButton
                    onClick={() => onApprove(item)}
                    aria-keyshortcuts="a"
                    aria-label={ui.message('Approve')}
                >
                    {ui.message('Approve')} <kbd aria-hidden="true">A</kbd>
                </DesignButton>
                <DesignButton
                    ref={rejectTrigger}
                    onClick={() => setRejecting(true)}
                    aria-keyshortcuts="r"
                    aria-label={ui.message('Reject')}
                >
                    {ui.message('Reject')} <kbd aria-hidden="true">R</kbd>
                </DesignButton>
                {item.kind !== 'image' && (
                    <DesignButton
                        onClick={() => setEditing(true)}
                        aria-keyshortcuts="e"
                        aria-label={ui.message('Modify and approve')}
                    >
                        {ui.message('Modify and approve')} <kbd aria-hidden="true">E</kbd>
                    </DesignButton>
                )}
            </footer>
            {rejecting && (
                <RejectPanel
                    item={item}
                    close={closeReject}
                    submit={(reason) => {
                        setRejecting(false)
                        onReject(item, reason)
                    }}
                />
            )}
            {editing && (
                <ApproveWithChanges
                    item={item}
                    close={() => setEditing(false)}
                    submit={(changes) => onApproveWithChanges(item, changes)}
                />
            )}
        </aside>
    )
}

function kindLabelFor(item: ReviewItem): string {
    return item.kind === 'marker'
        ? 'New place'
        : item.kind === 'edit'
          ? 'Content changes'
          : 'Photo submission'
}

function SubmitterStats({ item }: { item: ReviewItem }) {
    const ui = useAdminUi(),
        stats = item.stats
    if (!stats) return null
    return (
        <p className="review-stats">
            {ui.message('Approved')} {stats.approved} / {ui.message('Rejected')} {stats.rejected} /{' '}
            {ui.message('Pending')} {stats.pending}
        </p>
    )
}

function NewPlaceDetail({ item }: { item: Extract<ReviewItem, { kind: 'marker' }> }) {
    const ui = useAdminUi(),
        marker = item.marker,
        image = publicImageUrl(marker.markImage)
    return (
        <>
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
            <p className="admin-meta">
                {ui.message('Submitted by')} {marker.username}
            </p>
            <SubmitterStats item={item} />
            <PlaceMapPreview lat={marker.lat} lng={marker.lng} title={marker.title} />
            <a href={`/maps?markerId=${marker.id}`} target="_blank" rel="noreferrer">
                {ui.message('Open in map')}
            </a>
            {image && (
                <a href={image} target="_blank" rel="noreferrer">
                    <img className="admin-photo" src={image} alt={ui.message('Photo')} />
                </a>
            )}
        </>
    )
}

function EditDetail({ item }: { item: Extract<ReviewItem, { kind: 'edit' }> }) {
    const ui = useAdminUi(),
        work = useAdminWork(),
        edit = item.edit,
        current = useQuery({
            queryKey: [...work.prefix, 'current', edit.markerId, 'zh'],
            gcTime: 0,
            retry: false,
            queryFn: ({ signal }) =>
                work.run((s) => readAccountPlace(String(edit.markerId), 'zh', s), signal),
        })
    const rows = current.data ? comparisonRows(current.data, edit, ui.message) : []
    const changed = rows.filter((row) => row.changed)
    const unchanged = rows.filter((row) => !row.changed)
    return (
        <>
            <h3>{ui.message('Current place')}</h3>
            {current.isPending && <p role="status">{ui.message('Loading places…')}</p>}
            {current.isError && (
                <p role="alert">
                    {ui.message('This place is unavailable.')}{' '}
                    <DesignButton onClick={() => void current.refetch()}>
                        {ui.message('Retry')}
                    </DesignButton>
                </p>
            )}
            {current.data && (
                <>
                    <p className="review-current-title">{current.data.title}</p>
                    {current.data.contentLanguage !== edit.language && (
                        <p className="admin-meta">
                            {ui.message(
                                'Original content shown because this translation is unavailable.',
                            )}
                        </p>
                    )}
                    {changed.length ? (
                        <table className="review-compare">
                            <caption className="sr-only">{ui.message('Proposed changes')}</caption>
                            <thead>
                                <tr>
                                    <th scope="col">{ui.message('Field')}</th>
                                    <th scope="col">{ui.message('Current')}</th>
                                    <th scope="col">{ui.message('Proposed')}</th>
                                </tr>
                            </thead>
                            <tbody>
                                {changed.map((row) => (
                                    <tr key={row.key} className="review-changed">
                                        <th scope="row">{ui.message(row.label)}</th>
                                        <td>
                                            <span className="review-old">{row.current}</span>
                                            <span aria-hidden="true"> → </span>
                                            <span className="sr-only">
                                                {' '}
                                                {ui.message('changed to')}{' '}
                                            </span>
                                        </td>
                                        <td>
                                            <span className="review-new">{row.proposed}</span>
                                        </td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    ) : (
                        <p>{ui.message('No changes in this proposal.')}</p>
                    )}
                    {unchanged.length > 0 && (
                        <details className="review-unchanged">
                            <summary>
                                {unchanged.length} {ui.message('other fields unchanged')}
                            </summary>
                            <table className="review-compare">
                                <tbody>
                                    {unchanged.map((row) => (
                                        <tr key={row.key}>
                                            <th scope="row">{ui.message(row.label)}</th>
                                            <td>{row.current}</td>
                                            <td>{row.proposed}</td>
                                        </tr>
                                    ))}
                                </tbody>
                            </table>
                        </details>
                    )}
                </>
            )}
        </>
    )
}

function ImageDetail({ item }: { item: Extract<ReviewItem, { kind: 'image' }> }) {
    const ui = useAdminUi(),
        work = useAdminWork(),
        image = publicImageUrl(item.image.imageUrl),
        current = useQuery({
            queryKey: [...work.prefix, 'current', item.markerId, 'zh'],
            gcTime: 0,
            retry: false,
            queryFn: ({ signal }) =>
                work.run((s) => readAccountPlace(String(item.markerId), 'zh', s), signal),
        })
    return (
        <>
            {image ? (
                <a href={image} target="_blank" rel="noreferrer" className="review-image-link">
                    <img
                        className="admin-photo review-photo"
                        src={image}
                        alt={ui.message('Photo')}
                    />
                </a>
            ) : (
                <p>{ui.message('Image unavailable.')}</p>
            )}
            <p>
                <a href={`/maps?markerId=${item.markerId}`} target="_blank" rel="noreferrer">
                    {item.title}
                </a>
            </p>
            <p className="admin-meta">
                {ui.message('Submitted by')} {item.submitter}
            </p>
            <SubmitterStats item={item} />
            {current.data && current.data.photos?.length ? (
                <>
                    <h3>{ui.message('Existing photos')}</h3>
                    <ul className="review-album">
                        {current.data.photos.map((photo) => {
                            const src = publicImageUrl(photo.url)
                            return src ? (
                                <li key={photo.id}>
                                    <a href={src} target="_blank" rel="noreferrer">
                                        <img src={src} alt={ui.message('Photo')} />
                                    </a>
                                </li>
                            ) : null
                        })}
                    </ul>
                </>
            ) : null}
        </>
    )
}

export function RejectPanel({
    item,
    close,
    submit,
}: {
    item: ReviewItem
    close: () => void
    submit: (reason: string) => void
}) {
    const ui = useAdminUi(),
        [quick, setQuick] = useState(''),
        [text, setText] = useState(''),
        first = useRef<HTMLButtonElement>(null)
    useEffect(() => {
        first.current?.focus()
    }, [])
    useEffect(() => {
        const escape = (event: KeyboardEvent) => {
            if (event.key === 'Escape') {
                event.preventDefault()
                close()
            }
        }
        window.addEventListener('keydown', escape)
        return () => window.removeEventListener('keydown', escape)
    }, [close])
    const reasons = quickReasons.filter(
        (value) => value !== 'Inappropriate photo' || item.kind === 'image',
    )
    const reason = [quick, text.trim()].filter(Boolean).join(' · ')
    return (
        <section
            className="review-reject"
            aria-label={ui.message('Reject reason')}
            onKeyDown={(event) => {
                if (
                    event.key === 'Enter' &&
                    !event.shiftKey &&
                    event.target instanceof HTMLTextAreaElement
                ) {
                    event.preventDefault()
                    submit(reason)
                }
            }}
        >
            <h3>{ui.message('Reject reason')}</h3>
            <div className="admin-actions">
                {reasons.map((value, index) => (
                    <DesignButton
                        key={value}
                        ref={index === 0 ? first : undefined}
                        aria-pressed={quick === value}
                        onClick={() => setQuick(quick === value ? '' : value)}
                    >
                        {ui.message(value)}
                    </DesignButton>
                ))}
            </div>
            <label className="admin-field">
                {ui.message('Reason (optional)')}
                <textarea
                    rows={3}
                    maxLength={500}
                    value={text}
                    onChange={(event) => setText(event.target.value)}
                />
            </label>
            <p className="admin-meta">
                {ui.message('Enter confirms; Shift+Enter adds a line.')} {text.length}/500
            </p>
            <div className="admin-actions">
                <DesignButton onClick={() => submit(reason)}>
                    {ui.message('Confirm reject')}
                </DesignButton>
                <DesignButton onClick={close}>{ui.message('Cancel')}</DesignButton>
            </div>
        </section>
    )
}

function ApproveWithChanges({
    item,
    close,
    submit,
}: {
    item: ReviewItem
    close: () => void
    submit: (changes: MarkerText) => Promise<void>
}) {
    const ui = useAdminUi(),
        [draft, setDraft] = useState<MarkerText | null>(() => {
            if (item.kind === 'marker') return draftFromMarker(item.marker)
            return proposalDraft(item)
        }),
        [error, setError] = useState(''),
        [sending, setSending] = useState(false)
    if (!draft) return null
    return (
        <section className="review-editor" aria-label={ui.message('Modify and approve')}>
            <h3>{ui.message('Modify and approve')}</h3>
            <form
                onSubmit={(event) => {
                    event.preventDefault()
                    const result = markerTextSchema.safeParse(draft)
                    if (!result.success) {
                        setError('Check the title and both opening times.')
                        return
                    }
                    setError('')
                    setSending(true)
                    submit(result.data)
                        .catch((reason: unknown) => {
                            setError(moderationError(reason))
                        })
                        .finally(() => setSending(false))
                }}
            >
                <MarkerFields draft={draft} setDraft={setDraft} disabled={sending} />
                <label className="admin-field">
                    {ui.message('Visibility')}
                    <select
                        value={draft.isPublic ? 'public' : 'private'}
                        onChange={(event) =>
                            setDraft({ ...draft, isPublic: event.target.value === 'public' })
                        }
                    >
                        <option value="public">{ui.message('Public')}</option>
                        <option value="private">{ui.message('Private')}</option>
                    </select>
                </label>
                <label className="admin-field">
                    {ui.message('Language')}
                    <select
                        value={draft.language}
                        onChange={(event) =>
                            setDraft({
                                ...draft,
                                language: event.target.value === 'en' ? 'en' : 'zh',
                            })
                        }
                    >
                        <option value="zh">中文</option>
                        <option value="en">English</option>
                    </select>
                </label>
                <div className="admin-actions">
                    <DesignButton type="submit" disabled={sending}>
                        {ui.message('Approve with changes')}
                    </DesignButton>
                    <DesignButton onClick={close}>{ui.message('Cancel')}</DesignButton>
                </div>
                {error && <p role="alert">{ui.message(error)}</p>}
            </form>
        </section>
    )
}
