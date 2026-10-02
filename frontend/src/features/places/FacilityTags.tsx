import type { Marker } from '@/shared/api/markers'
import { useUi } from '@/shared/i18n/ui'
import { categoryLabels } from './model'
export function FacilityTags({ place }: { place: Pick<Marker, 'category' | 'categories'> }) {
    const ui = useUi(),
        values = place.categories ?? [place.category]
    if (values.length < 2) return null
    return (
        <>
            {values.map((value) => (
                <span key={value} className={`place-tag facility-${value}`}>
                    {ui.message(categoryLabels[value])}
                </span>
            ))}
        </>
    )
}
