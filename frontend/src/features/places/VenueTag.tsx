import type { Marker } from '@/shared/api/markers'
import { useUi } from '@/shared/i18n/ui'
import { venueLabels } from './venue'
import './place-tags.css'

export function VenueTag({
    place,
}: {
    place: Pick<Marker, 'category' | 'categories' | 'venueType'>
}) {
    const ui = useUi()
    // An older server's missing field is not evidence of a classification.
    if (!(place.categories ?? [place.category]).includes('accessible_toilet') || !place.venueType)
        return null
    return (
        <span className={`place-tag venue-${place.venueType}`}>
            {ui.message(venueLabels[place.venueType])}
        </span>
    )
}
