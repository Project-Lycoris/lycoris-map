import 'leaflet/dist/leaflet.css'
import { useCallback, useEffect, useRef, useState, type RefCallback } from 'react'
import { AttributionControl, MapContainer, TileLayer, useMap } from 'react-leaflet'
import L, { type Map as LeafletMap } from 'leaflet'
import { MapPlaces, type MapPlacesProps } from './MapPlaces'
import { usePreferences } from '@/features/preferences/PreferencesProvider'
import { isMapSourceAvailable, osmTileUrl, tiandituTileUrl } from './mapSources'
import TencentBaseMap from './tencent/TencentBaseMap'
import { watchTileLayer } from './watchTileLayer'
import { scheduleIdlePreload } from './idlePreload'
import { loadTencentResources } from './tencent/resources'
import { mapAttributionPrefix } from '@/shared/siteFilings'
import '@/shared/site-filings.css'
const CENTER: [number, number] = [31.2304, 121.4737]
export function MapSurface({
    onMap,
    onPick,
    places,
}: {
    onMap: RefCallback<LeafletMap>
    onPick?: ((point: { lat: number; lng: number }) => void) | undefined
    places?: MapPlacesProps | undefined
}) {
    return (
        <MapContainer
            ref={onMap}
            center={CENTER}
            zoom={14}
            zoomControl={false}
            trackResize={false}
            maxZoom={19}
            className="product-map"
            attributionControl={false}
        >
            <AttributionControl prefix={mapAttributionPrefix} />
            <BaseMapLayers />
            <MapLifecycle />
            <MapPick onPick={onPick} />
            {places && <MapPlaces {...places} />}
        </MapContainer>
    )
}
function BaseMapLayers() {
    const { preferences, reportSourceFailure, sourceAttempt } = usePreferences()
    const monitorKey = `${sourceAttempt}/${preferences.source}`
    const [ready, setReady] = useState<string | null>(null)
    const loaded = useCallback(() => setReady(monitorKey), [monitorKey])
    const failed = useCallback(
        () => reportSourceFailure(preferences.source),
        [reportSourceFailure, preferences.source],
    )
    useEffect(() => {
        if (
            ready !== monitorKey ||
            preferences.source === 'tencent' ||
            !isMapSourceAvailable('tencent')
        )
            return
        return scheduleIdlePreload(() => loadTencentResources(true))
    }, [ready, monitorKey, preferences.source])
    if (preferences.source === 'tencent')
        return <TencentBaseMap key={monitorKey} onError={failed} onReady={loaded} />
    if (preferences.source === 'tianditu')
        return (
            <>
                <MonitoredTileLayer
                    key={`${monitorKey}/base`}
                    onError={failed}
                    onReady={loaded}
                    url={tiandituTileUrl('vec')}
                    subdomains="01234567"
                    minNativeZoom={1}
                    maxNativeZoom={18}
                    maxZoom={19}
                    zIndex={1}
                    attribution='&copy; <a href="https://www.tianditu.gov.cn/">天地图</a>'
                />
                <MonitoredTileLayer
                    key={`${monitorKey}/labels`}
                    onError={failed}
                    onReady={loaded}
                    url={tiandituTileUrl('cva')}
                    subdomains="01234567"
                    minNativeZoom={1}
                    maxNativeZoom={18}
                    maxZoom={19}
                    zIndex={2}
                />
            </>
        )
    return (
        <MonitoredTileLayer
            key={monitorKey}
            onError={failed}
            onReady={loaded}
            url={osmTileUrl}
            maxNativeZoom={19}
            maxZoom={19}
            attribution='&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors'
        />
    )
}
function MonitoredTileLayer({
    onError,
    onReady,
    ...props
}: React.ComponentProps<typeof TileLayer> & { onError: () => void; onReady: () => void }) {
    const cleanup = useRef<(() => void) | undefined>(undefined)
    const actions = useRef({ onError, onReady })
    actions.current = { onError, onReady }
    const ref = useCallback((layer: L.TileLayer | null) => {
        cleanup.current?.()
        cleanup.current = layer
            ? watchTileLayer(
                  layer,
                  () => actions.current.onError(),
                  () => actions.current.onReady(),
              )
            : undefined
    }, [])
    return <TileLayer {...props} ref={ref} />
}
function MapPick({
    onPick,
}: {
    onPick: ((point: { lat: number; lng: number }) => void) | undefined
}) {
    const map = useMap()
    useEffect(() => {
        const container = map.getContainer()
        const oldLabel = container.getAttribute('aria-label')
        container.setAttribute(
            'aria-label',
            onPick ? 'Map. Click a location, or press Enter to choose the map center.' : 'Map',
        )
        const restoreLabel = () => {
            if (oldLabel === null) container.removeAttribute('aria-label')
            else container.setAttribute('aria-label', oldLabel)
        }
        if (!onPick) return restoreLabel
        const choose = (point: L.LatLng) => {
            const { lat, lng } = point.wrap()
            onPick({ lat, lng })
        }
        const click = (event: L.LeafletMouseEvent) => choose(event.latlng)
        const keyboard = (event: KeyboardEvent) => {
            if (event.key === 'Enter' && event.target === map.getContainer()) {
                event.preventDefault()
                choose(map.getCenter())
            }
        }
        map.on('click', click)
        map.getContainer().addEventListener('keydown', keyboard)
        return () => {
            map.off('click', click)
            map.getContainer().removeEventListener('keydown', keyboard)
            restoreLabel()
        }
    }, [map, onPick])
    return null
}
function MapLifecycle() {
    const map = useMap()
    useEffect(() => {
        const container = map.getContainer()
        // Read-only DOM evidence for development/real-Leaflet regression tests.
        const report = () => {
            if (!import.meta.env.DEV) return
            container.dataset.mapId = String(L.stamp(map))
            container.dataset.zoom = String(map.getZoom())
            container.dataset.center = `${map.getCenter().lat},${map.getCenter().lng}`
        }
        let width = 0
        let height = 0
        // Tracks whether Leaflet has ever received a valid non-zero size.
        let sized = false
        // A pending restore only redraws tiles when the browser actually
        // restored the page (BFCache/visibility), never on ordinary layout
        // changes. Pan/zoom must not trigger a full tile redraw.
        let restorePending = false
        let frame = 0
        const flush = () => {
            frame = 0
            const nextWidth = container.clientWidth
            const nextHeight = container.clientHeight
            const sizeChanged = nextWidth !== width || nextHeight !== height
            if (nextWidth === 0 || nextHeight === 0) {
                // Leaflet cannot compute a meaningful size for a hidden or
                // unmeasured container; wait for the next valid measurement.
                restorePending = true
                return
            }
            if (sizeChanged) map.invalidateSize({ pan: true, animate: false })
            if (!sized || restorePending) {
                // Restored from hidden/BFCache without a size change: Leaflet's
                // internal size and tile cache can still be stale, so resync and
                // redraw once. This is the only path that reloads all tiles.
                if (!sizeChanged) map.invalidateSize({ pan: false, animate: false })
                map.eachLayer((layer) => {
                    if (layer instanceof L.TileLayer) layer.redraw()
                })
            }
            width = nextWidth
            height = nextHeight
            sized = true
            restorePending = false
        }
        const schedule = () => {
            if (frame) return
            frame = requestAnimationFrame(flush)
        }
        const resize = new ResizeObserver(() => {
            const nextWidth = container.clientWidth
            const nextHeight = container.clientHeight
            if (nextWidth === width && nextHeight === height && !restorePending) return
            // One resize owner; Leaflet's public pan compensation keeps the
            // camera within its half-pixel rounding across odd/even widths.
            schedule()
        })
        const visibility = () => {
            if (document.visibilityState === 'visible') {
                restorePending = true
                schedule()
            }
        }
        const pageshow = (event: PageTransitionEvent) => {
            // `persisted` marks a BFCache restore, where neither ResizeObserver
            // nor a resize event is guaranteed to fire.
            if (event.persisted) {
                restorePending = true
                schedule()
            }
        }
        resize.observe(container)
        // Prime the tracked size. Leaflet itself already measured a non-zero
        // mount, so only a 0x0 mount counts as "not yet sized"; a transient
        // 0x0 after that is treated as a hidden restore.
        width = container.clientWidth
        height = container.clientHeight
        sized = width > 0 && height > 0
        report()
        map.on('moveend zoomend', report)
        document.addEventListener('visibilitychange', visibility)
        window.addEventListener('pageshow', pageshow)
        return () => {
            resize.disconnect()
            if (frame) cancelAnimationFrame(frame)
            map.off('moveend zoomend', report)
            document.removeEventListener('visibilitychange', visibility)
            window.removeEventListener('pageshow', pageshow)
        }
    }, [map])
    return null
}
