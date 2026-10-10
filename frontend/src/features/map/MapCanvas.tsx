import 'leaflet/dist/leaflet.css'
import { useCallback, useEffect, useRef, useState } from 'react'
import {
    AttributionControl,
    CircleMarker,
    MapContainer,
    TileLayer,
    Tooltip,
    useMap,
} from 'react-leaflet'
import L, { type Map as LeafletMap } from 'leaflet'
import type { Language } from '@/shared/i18n'
import { resolveTitle, type SyntheticMarker } from './syntheticMarkers'
import { toLeafletTuple } from './coords'
import { mapAttributionPrefix } from '@/shared/siteFilings'
import '@/shared/site-filings.css'

const TILE_URL = '/tiles/osm/{z}/{x}/{y}.png'
const OSM_ATTRIBUTION =
    '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors'

export const SHANGHAI_CENTER: [number, number] = [31.2304, 121.4737]
export const INITIAL_ZOOM = 14

/**
 * Stable identity for a real Leaflet map object.
 *
 * Keyed by the map instance itself, so re-running an effect for the *same* map
 * returns the same id. A counter that increments per effect run would report a
 * new "instance" for a replayed effect, which is exactly the false evidence we
 * must avoid.
 */
const instanceIds = new WeakMap<LeafletMap, string>()

function instanceIdOf(map: LeafletMap): string {
    const existing = instanceIds.get(map)
    if (existing) return existing
    const id = `map-${L.stamp(map)}`
    instanceIds.set(map, id)
    return id
}

type MapCanvasProps = {
    markers: readonly SyntheticMarker[]
    language: Language
}

type ViewState = { center: [number, number]; zoom: number }

/**
 * Persistent OSM map surface.
 *
 * Deliberately NOT keyed by `language`, panel state, filters or auth: those
 * update marker props/panels, and the Leaflet instance must survive them.
 * `MapContainer` is mounted once at this position in the tree.
 */
export function MapCanvas({ markers, language }: MapCanvasProps) {
    const mapRef = useRef<LeafletMap | null>(null)
    const [instanceId, setInstanceId] = useState('')
    const [view, setView] = useState<ViewState>({ center: SHANGHAI_CENTER, zoom: INITIAL_ZOOM })

    /**
     * Stable identity callback. `handleInstance` must not be recreated on every
     * render, otherwise the probe effect would re-run and set state in a loop.
     */
    const handleInstance = useCallback((map: LeafletMap | null) => {
        setInstanceId(map ? instanceIdOf(map) : '')
    }, [])

    const handleViewChange = useCallback((next: ViewState) => {
        setView(next)
    }, [])

    return (
        <div className="relative h-full w-full">
            <MapContainer
                ref={mapRef}
                center={SHANGHAI_CENTER}
                zoom={INITIAL_ZOOM}
                className="h-full w-full"
                // No extra Leaflet controls; S2 designs the real UI chrome.
                zoomControl={false}
                attributionControl={false}
            >
                <AttributionControl prefix={mapAttributionPrefix} />
                <TileLayer url={TILE_URL} attribution={OSM_ATTRIBUTION} />
                <MapInstanceProbe onInstance={handleInstance} />
                <ViewReporter onChange={handleViewChange} />
                {markers.map((marker) => (
                    <CircleMarker
                        key={marker.id}
                        center={toLeafletTuple(marker)}
                        radius={6}
                        pathOptions={{
                            color: marker.isActive ? '#461F3A' : '#EFB8C8',
                            fillColor: marker.isActive ? '#5A3850' : '#EFB8C8',
                            fillOpacity: 0.7,
                        }}
                    >
                        <Tooltip>
                            {resolveTitle(marker, language)} · v{marker.version}
                        </Tooltip>
                    </CircleMarker>
                ))}
            </MapContainer>
            <MapEvidenceOverlay instanceId={instanceId} markerCount={markers.length} view={view} />
        </div>
    )
}

/**
 * Reports the live Leaflet identity via `WeakMap`/`L.stamp`, not a per-effect
 * counter. Called once per real map object by `MapContainer`; no state write
 * happens during render.
 */
function MapInstanceProbe({ onInstance }: { onInstance: (map: LeafletMap | null) => void }) {
    const map = useMap()
    useEffect(() => {
        onInstance(map)
        return () => onInstance(null)
    }, [map, onInstance])
    return null
}

function ViewReporter({ onChange }: { onChange: (view: ViewState) => void }) {
    const map = useMap()
    useEffect(() => {
        const report = () => {
            const center = map.getCenter()
            onChange({ center: [center.lat, center.lng], zoom: map.getZoom() })
        }
        const resetView = () => {
            map.setView(SHANGHAI_CENTER, INITIAL_ZOOM)
            report()
        }
        report()
        map.on('moveend', report)
        map.on('zoomend', report)
        window.addEventListener('lycoris:spike-reset-view', resetView)
        // Paired cleanup: no Leaflet listener or window listener is left behind.
        return () => {
            map.off('moveend', report)
            map.off('zoomend', report)
            window.removeEventListener('lycoris:spike-reset-view', resetView)
        }
    }, [map, onChange])
    return null
}

/** Visible evidence text; no `window` debug hooks are exposed. */
function MapEvidenceOverlay({
    instanceId,
    markerCount,
    view,
}: {
    instanceId: string
    markerCount: number
    view: ViewState
}) {
    const [lat, lng] = view.center
    return (
        <div
            data-testid="map-evidence"
            className="pointer-events-none absolute right-2 bottom-6 z-[500] rounded-md px-2 py-1 text-xs"
            style={{ background: 'var(--ui-nav-surface)', color: 'var(--ui-brand-deep)' }}
        >
            <div>markers: {markerCount}</div>
            <div>instance: {instanceId || 'creating'}</div>
            <div>
                center: {lat.toFixed(5)}, {lng.toFixed(5)}
            </div>
            <div>zoom: {view.zoom}</div>
        </div>
    )
}

/** Exposed for the lifecycle test to assert the instance did not change. */
export function readMapInstanceId(container: HTMLElement): string {
    const evidence = container.querySelector('[data-testid="map-evidence"]')
    const text = evidence?.textContent ?? ''
    const match = /instance:\s*(\S+)/.exec(text)
    return match?.[1] ?? ''
}

/** Exposed for the lifecycle test: the Leaflet container element itself. */
export function readLeafletContainer(container: HTMLElement): Element | null {
    return container.querySelector('.leaflet-container')
}
