import 'leaflet/dist/leaflet.css'
import { CircleMarker, MapContainer, TileLayer } from 'react-leaflet'
import { osmTileUrl } from '@/features/map/mapSources'
import { useAdminUi } from './ui'

/** Non-interactive location check for a new-place submission. */
export function PlaceMapPreview({ lat, lng, title }: { lat: number; lng: number; title: string }) {
    const ui = useAdminUi()
    return (
        <div
            className="review-map"
            role="img"
            aria-label={`${ui.message('Location preview')}: ${title}`}
        >
            <MapContainer
                center={[lat, lng]}
                zoom={16}
                dragging={false}
                scrollWheelZoom={false}
                doubleClickZoom={false}
                touchZoom={false}
                keyboard={false}
                zoomControl={false}
                attributionControl={false}
                className="review-map-canvas"
            >
                <TileLayer url={osmTileUrl} />
                <CircleMarker
                    center={[lat, lng]}
                    radius={9}
                    pathOptions={{ color: '#65558f', fillColor: '#eaddff', fillOpacity: 1 }}
                />
            </MapContainer>
        </div>
    )
}
