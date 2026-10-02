import { useUi } from '@/shared/i18n/ui'
import { useEffect, useRef } from 'react'
import { IconButton } from '@/shared/ui/design-primitives'
import { useOpeningStatus, hoursStatusLabel } from './openingStatus'
import { VenueTag } from './VenueTag'
import { distanceLabel, openingHours } from './model'
import type { PlaceBrowse } from './usePlaceBrowse'
import { ReadMessage } from './PlaceResults'
import { PlaceActions } from './PlaceActions'
import { BookmarkButton } from '@/features/bookmarks/BookmarkButton'
import { PlaceGallery } from './PlaceGallery'
import { FacilityTags } from './FacilityTags'

export function PlaceDetails({
    browse,
    mobile = false,
    onEdit,
}: {
    browse: PlaceBrowse
    mobile?: boolean
    onEdit?: (() => void) | undefined
}) {
    const ui = useUi()
    const { detail: place, detailState } = browse
    const hours = useOpeningStatus(place)
    const heading = useRef<HTMLHeadingElement>(null)
    useEffect(() => {
        heading.current?.focus({ preventScroll: true })
    }, [place?.id])
    const distance = place ? distanceLabel(place, browse.location.position) : null
    return (
        <div
            className={
                mobile
                    ? 'mobile-detail-content live-mobile-detail'
                    : 'desktop-place-detail live-place-detail'
            }
        >
            {!place || detailState.error ? (
                <ReadMessage state={detailState} />
            ) : (
                <>
                    {mobile ? (
                        <h1 ref={heading} tabIndex={-1} lang={place.contentLanguage}>
                            {place.title}
                        </h1>
                    ) : (
                        <h2 ref={heading} tabIndex={-1} lang={place.contentLanguage}>
                            {place.title}
                        </h2>
                    )}
                    {!mobile ? (
                        <BookmarkButton place={place} language={browse.language} />
                    ) : (
                        <IconButton
                            className={mobile ? 'mobile-place-edit' : 'details-bookmark'}
                            id="mobile-place-edit"
                            icon={mobile ? 'mobileEdit' : 'navBookmarks'}
                            size={mobile ? 20 : 24}
                            label={mobile ? 'Edit place' : 'Bookmark place'}
                            available={!!onEdit}
                            onClick={onEdit}
                        />
                    )}
                    <div className="place-tags">
                        <FacilityTags place={place} />
                        <VenueTag place={place} />
                        {hours === 'closing-soon' && (
                            <span className="place-tag closing-soon-tag">
                                {ui.text('Closing soon')}
                            </span>
                        )}
                    </div>
                    <span className="place-meta">
                        {distance && (
                            <span title={ui.text('Straight-line distance from your location')}>
                                {distance}
                            </span>
                        )}
                        <span className={`place-hours place-hours-${hours}`}>
                            {hoursStatusLabel(hours) && (
                                <>{ui.message(hoursStatusLabel(hours))} · </>
                            )}
                            {place.openingHoursNote?.trim()
                                ? ui.text('See opening hours note')
                                : openingHours(place, browse.language)}
                        </span>
                    </span>
                    {mobile && place.description && (
                        <p className="mobile-description" lang={place.contentLanguage}>
                            {place.description}
                        </p>
                    )}
                    {place.openingHoursNote && (
                        <p className="place-hours-note">{place.openingHoursNote}</p>
                    )}
                    <PlaceGallery key={place.id} place={place} mobile={mobile} />
                    {!mobile && place.description && (
                        <p className="place-description" lang={place.contentLanguage}>
                            {place.description}
                        </p>
                    )}
                    <PlaceActions place={place} language={browse.language} mobile={mobile}>
                        {mobile && (
                            <BookmarkButton place={place} language={browse.language} mobile />
                        )}
                    </PlaceActions>
                </>
            )}
        </div>
    )
}
