import { useState } from 'react'
import type { Marker } from '@/shared/api/markers'
import { DesignButton } from '@/shared/ui/design-primitives'
import { useUi } from '@/shared/i18n/ui'
import { publicImageUrl } from './model'
import { PlacePhoto } from './PlacePhoto'
export function PlaceGallery({ place, mobile }: { place: Marker; mobile: boolean }) {
    const ui = useUi(),
        [selected, setSelected] = useState(0)
    const photos = (
        place.photos?.length
            ? place.photos.map((photo) => photo.url)
            : place.markImage
              ? [place.markImage]
              : []
    ).flatMap((url) => {
        const safe = publicImageUrl(url)
        return safe ? [safe] : []
    })
    if (!photos.length) return null
    const index = Math.min(selected, photos.length - 1)
    return (
        <div className="place-gallery">
            <PlacePhoto
                key={`${place.id}:${photos[index]}`}
                className={mobile ? 'mobile-photo' : 'place-photo'}
                src={photos[index]!}
            />
            {photos.length > 1 && (
                <div className="place-gallery-controls" aria-label={ui.text('Photos')}>
                    <DesignButton
                        aria-label={ui.text('Previous photo')}
                        disabled={index === 0}
                        onClick={() => setSelected(index - 1)}
                    >
                        ‹
                    </DesignButton>
                    <span aria-live="polite">
                        {index + 1} / {photos.length}
                    </span>
                    <DesignButton
                        aria-label={ui.text('Next photo')}
                        disabled={index === photos.length - 1}
                        onClick={() => setSelected(index + 1)}
                    >
                        ›
                    </DesignButton>
                </div>
            )}
        </div>
    )
}
