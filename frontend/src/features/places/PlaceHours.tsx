import type { Marker } from '@/shared/api/markers'
import type { Language } from '@/shared/query/keys'
import { useUi } from '@/shared/i18n/ui'
import { openingHours } from './model'
import { useOpeningStatus } from './openingStatus'
import './place-tags.css'

export function PlaceHours({ place, language }: { place: Marker; language: Language }) {
    const ui = useUi()
    const status = useOpeningStatus(place)
    return (
        <span className={`place-hours place-hours-${status}`}>
            {status === 'closing-soon' && <>{ui.text('Closing soon')} · </>}
            {place.openingHoursNote?.trim()
                ? ui.text('See opening hours note')
                : openingHours(place, language)}
        </span>
    )
}
