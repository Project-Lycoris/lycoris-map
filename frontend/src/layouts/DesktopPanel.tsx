import { isSettingsPanel, settingsTitles, SettingsContent } from '@/features/preferences/Settings'
import { useUi } from '@/shared/i18n/ui'
import { useCallback, useState } from 'react'
import { FigmaIcon } from '@/shared/ui/figma-icon'
import type { VoiceSearchState } from '@/shared/ui/design-primitives'
import { CategoryBadge, DesignButton, IconButton, NearbyCards, SearchField } from './primitives'
import type { DesignSample, Panel } from './types'
import { ContributionForm, type ContributionFormProps } from './ContributionForm'
import type { Marker } from '@/shared/api/markers'
import type { PlaceBrowse } from '@/features/places/usePlaceBrowse'
import { PlaceDetails } from '@/features/places/PlaceDetails'
import { NearbyResults } from '@/features/places/NearbyResults'
import { PlaceResults } from '@/features/places/PlaceResults'
import { BookmarksPanel } from '@/features/bookmarks/BookmarksPanel'

type Props = {
    panel: Panel
    sample: DesignSample | undefined
    search: string
    bookmarksSearch: string
    setSearch: (value: string) => void
    setBookmarksSearch: (value: string) => void
    open: (panel: Panel, focusId?: string) => void
    close: () => void
    contribution?: Omit<ContributionFormProps, 'mobile'>
    browse?: PlaceBrowse | undefined
    selectPlace?: ((place: Marker, focusId: string) => void) | undefined
    chooseCategory?: ((category: 'toilet' | 'nursing' | 'medical') => void) | undefined
    editPlace?: (() => void) | undefined
}

export function DesktopPanel(props: Props) {
    const ui = useUi()
    const { panel, sample, close, open } = props
    const [voice, setVoice] = useState(false)
    const onVoiceChange = useCallback((next: VoiceSearchState) => {
        setVoice(next.active || next.finishing)
    }, [])
    if (panel === 'initial') return null
    if (panel === 'contribute-form' && props.contribution)
        return (
            <section
                className="desktop-panel panel-contribute-form"
                aria-label={ui.text('Contribute')}
            >
                <ContributionForm {...props.contribution} />
            </section>
        )
    if (panel === 'contribute') return null
    const heading = isSettingsPanel(panel)
        ? settingsTitles[panel]
        : panel.charAt(0).toUpperCase() + panel.slice(1)
    return (
        <section className={`desktop-panel panel-${panel}`} aria-label={ui.message(heading)}>
            <h1>{ui.message(heading)}</h1>
            <IconButton className="panel-close" icon="close" label="Close panel" onClick={close} />
            {panel === 'search' && (
                <>
                    <SearchField
                        value={props.search}
                        onChange={props.setSearch}
                        onVoiceChange={onVoiceChange}
                    />
                    {voice ? null : props.browse &&
                      (props.browse.mode === 'search' || props.browse.mode === 'cluster') &&
                      props.selectPlace ? (
                        <PlaceResults browse={props.browse} onSelect={props.selectPlace} />
                    ) : (
                        <>
                            <h2 className="nearby-heading">{ui.text('Find Nearby')}</h2>
                            <NearbyCards onSelect={props.chooseCategory} />
                        </>
                    )}
                </>
            )}
            {panel === 'nearby' && props.browse && props.selectPlace && (
                <NearbyResults browse={props.browse} onSelect={props.selectPlace} />
            )}
            {panel === 'bookmarks' && !sample && props.browse && props.selectPlace && (
                <BookmarksPanel browse={props.browse} onSelect={props.selectPlace} />
            )}
            {panel === 'bookmarks' && sample && (
                <>
                    <SearchField
                        bookmarks
                        value={props.bookmarksSearch}
                        onChange={props.setBookmarksSearch}
                    />
                    <h2 className="nearby-heading">{ui.text('Nearest Locations')}</h2>
                    {sample && (
                        <div className="nearby-cards">
                            <DesignButton
                                id="desktop-bookmark-place"
                                className="category-card bookmark-card"
                                onClick={() => open('details', 'desktop-bookmark-place')}
                            >
                                <CategoryBadge category="toilet" />
                                <PlaceSummary place={sample.place} />
                            </DesignButton>
                            <DesignButton className="category-card" available={false}>
                                <CategoryBadge category="nursing" />
                                <span className="card-title">{ui.text('Nursing Rooms')}</span>
                            </DesignButton>
                            <DesignButton className="category-card" available={false}>
                                <CategoryBadge category="medical" />
                                <span className="card-title">
                                    {ui.text('Medical Institutions')}
                                </span>
                            </DesignButton>
                        </div>
                    )}
                </>
            )}
            {isSettingsPanel(panel) && <SettingsContent panel={panel} />}
            {panel === 'details' && (
                <>
                    <IconButton
                        className="details-edit"
                        id="desktop-place-edit"
                        icon="edit"
                        size={20}
                        label="Edit place"
                        available={!!props.editPlace && !!props.browse?.detail}
                        onClick={props.editPlace}
                    />
                    {props.browse ? (
                        <PlaceDetails browse={props.browse} />
                    ) : (
                        sample && (
                            <div className="desktop-place-detail">
                                <h2>{sample.place.title.replace(' Shanghai,', '\nShanghai,')}</h2>
                                <IconButton
                                    className="details-bookmark"
                                    icon="bookmarkFilled"
                                    label="Bookmark place"
                                    available={false}
                                />
                                <PlaceMeta place={sample.place} />
                                <img
                                    className="place-photo"
                                    src={sample.place.desktopPhoto}
                                    alt=""
                                />
                                <p className="place-description">
                                    {sample.place.desktopDescription}
                                </p>
                            </div>
                        )
                    )}
                    {!props.browse && (
                        <>
                            <ShareButton />
                            <DesignButton className="navigate-button" available={false}>
                                <span>{ui.text('Navigate')}</span>
                                <FigmaIcon name="forward" />
                            </DesignButton>
                        </>
                    )}
                </>
            )}
        </section>
    )
}

export function PlaceMeta({ place }: { place: DesignSample['place'] }) {
    return (
        <span className="place-meta">
            <span>{place.distance}</span>
            <span>{place.hours}</span>
        </span>
    )
}
export function PlaceSummary({ place }: { place: DesignSample['place'] }) {
    return (
        <span className="place-summary">
            <span className="card-title">{place.bookmarkTitle}</span>
            <PlaceMeta place={place} />
        </span>
    )
}
export function ShareButton({ mobile = false }: { mobile?: boolean }) {
    const ui = useUi()
    return (
        <DesignButton className="share-button" available={false}>
            <span>{ui.text('Share')}</span>
            <FigmaIcon name={mobile ? 'mobileShare' : 'share'} />
        </DesignButton>
    )
}
