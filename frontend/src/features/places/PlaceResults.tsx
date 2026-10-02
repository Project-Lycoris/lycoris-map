import { PlaceHours } from '@/features/places/PlaceHours'
import { useUi } from '@/shared/i18n/ui'
import { useRef, useState } from 'react'
import type { Marker } from '@/shared/api/markers'
import { CategoryBadge, DesignButton } from '@/shared/ui/design-primitives'
import { FigmaIcon } from '@/shared/ui/figma-icon'
import { categoryBadges, categoryLabels, distanceLabel } from './model'
import type { PlaceBrowse, PlaceReadState } from './usePlaceBrowse'

export function ReadMessage({ state, empty = false }: { state: PlaceReadState; empty?: boolean }) {
    const ui = useUi()
    return (
        <div className="place-read-message" role="status" aria-live="polite">
            {ui.message(
                state.error ??
                    (state.pending ? 'Loading places…' : empty ? 'No places found.' : ''),
            )}
            {state.error && state.retryable !== false && (
                <DesignButton className="read-retry" onClick={state.retry}>
                    {ui.text('Try again')}
                </DesignButton>
            )}
        </div>
    )
}

export function PlaceResults({
    browse,
    onSelect,
    mobile = false,
}: {
    browse: PlaceBrowse
    onSelect: (place: Marker, focusId: string) => void
    mobile?: boolean
}) {
    const listKey = JSON.stringify([
        mobile,
        browse.mode,
        browse.search.trim(),
        browse.nearby,
        browse.language,
        browse.clusterIds,
    ])
    return (
        <ResultList
            key={listKey}
            browse={browse}
            onSelect={onSelect}
            mobile={mobile}
            listKey={listKey}
        />
    )
}
function ResultList({
    browse,
    onSelect,
    mobile,
    listKey,
}: {
    browse: PlaceBrowse
    onSelect: (place: Marker, focusId: string) => void
    mobile: boolean
    listKey: string
}) {
    const ui = useUi()
    const { results, state } = browse
    const saved = browse.listPositions.current.get(listKey)
    const scroller = useRef<HTMLDivElement>(null)
    const [offset, setOffset] = useState(saved?.offset ?? 0)
    const [active, setActive] = useState(saved?.active ?? 0)
    const remember = (nextOffset: number, nextActive: number) => {
        browse.listPositions.current.set(listKey, { offset: nextOffset, active: nextActive })
        setOffset(nextOffset)
        setActive(nextActive)
    }
    const virtual = results.length > 100
    const stride = mobile ? 67 : 82
    const start = virtual
        ? Math.max(0, Math.min(results.length - 1, Math.floor(offset / stride)) - 4)
        : 0
    const end = virtual
        ? Math.min(
              results.length,
              start + Math.ceil((scroller.current?.clientHeight || 800) / stride) + 8,
          )
        : results.length
    const heading =
        browse.mode === 'search'
            ? 'Search Results'
            : browse.mode === 'nearby' && browse.nearby
              ? categoryLabels[browse.nearby.category]
              : 'Locations'
    return (
        <section
            className={`place-results ${mobile ? 'mobile-place-results' : ''}`}
            aria-label={ui.message(heading) ?? undefined}
        >
            <h2 className="results-heading">{ui.message(heading)}</h2>
            {browse.mode === 'nearby' && (
                <p className="nearby-reference">
                    Within 1km of {browse.nearby?.located ? 'your location' : 'the map center'}
                </p>
            )}
            <ReadMessage state={state} empty={results.length === 0} />
            {results.length > 0 && (
                <div
                    ref={(element) => {
                        scroller.current = element
                        if (element) element.scrollTop = offset
                    }}
                    className="place-result-scroll"
                    role="list"
                    aria-label={ui.message(heading) ?? undefined}
                    tabIndex={virtual ? 0 : undefined}
                    onScroll={(event) => remember(event.currentTarget.scrollTop, active)}
                    onKeyDown={(event) => {
                        if (
                            !virtual ||
                            !['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)
                        )
                            return
                        event.preventDefault()
                        const current = Number(
                            (event.target as HTMLElement).closest<HTMLElement>('[data-row]')
                                ?.dataset.row ?? active,
                        )
                        const next =
                            event.key === 'Home'
                                ? 0
                                : event.key === 'End'
                                  ? results.length - 1
                                  : Math.max(
                                        0,
                                        Math.min(
                                            results.length - 1,
                                            current + (event.key === 'ArrowDown' ? 1 : -1),
                                        ),
                                    )
                        const list = event.currentTarget
                        const top = next * stride
                        if (
                            top < list.scrollTop ||
                            top + stride > list.scrollTop + list.clientHeight
                        ) {
                            list.scrollTop = top
                        }
                        remember(list.scrollTop, next)
                        requestAnimationFrame(() =>
                            list
                                .querySelector<HTMLButtonElement>(`[data-row="${next}"]`)
                                ?.focus({ preventScroll: true }),
                        )
                    }}
                >
                    <div
                        style={
                            virtual
                                ? { height: results.length * stride, position: 'relative' }
                                : undefined
                        }
                    >
                        {results.slice(start, end).map((place, index) => {
                            const badge = categoryBadges[place.category]
                            const distance = distanceLabel(place, browse.location.position)
                            const row = start + index
                            const id = `${mobile ? 'mobile' : 'desktop'}-result-${place.id}`
                            return (
                                <div
                                    key={place.id}
                                    role="listitem"
                                    aria-setsize={results.length}
                                    aria-posinset={row + 1}
                                    style={
                                        virtual
                                            ? {
                                                  position: 'absolute',
                                                  top: row * stride,
                                                  left: 0,
                                                  right: 0,
                                              }
                                            : undefined
                                    }
                                >
                                    <DesignButton
                                        id={id}
                                        data-row={row}
                                        className={`place-result-row ${mobile ? 'mobile-place-row' : 'category-card'}`}
                                        tabIndex={
                                            virtual
                                                ? row === Math.min(active, results.length - 1)
                                                    ? 0
                                                    : -1
                                                : 0
                                        }
                                        onClick={() => {
                                            remember(scroller.current?.scrollTop ?? offset, row)
                                            onSelect(place, id)
                                        }}
                                    >
                                        {badge ? (
                                            <CategoryBadge category={badge} />
                                        ) : (
                                            <span className="category-badge badge-custom">
                                                <FigmaIcon name="map" size={20} />
                                            </span>
                                        )}
                                        <span
                                            className="place-summary"
                                            lang={place.contentLanguage}
                                        >
                                            <span className="card-title">{place.title}</span>
                                            <span className="place-meta">
                                                {distance && (
                                                    <span
                                                        title={ui.text(
                                                            'Straight-line distance from your location',
                                                        )}
                                                    >
                                                        {distance}
                                                    </span>
                                                )}
                                                <PlaceHours
                                                    place={place}
                                                    language={browse.language}
                                                />
                                            </span>
                                        </span>
                                    </DesignButton>
                                </div>
                            )
                        })}
                    </div>
                </div>
            )}
        </section>
    )
}
