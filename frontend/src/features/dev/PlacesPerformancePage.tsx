import { useEffect, useMemo, useRef, useState } from 'react'
import { QueryClient, QueryClientProvider, useQueryClient } from '@tanstack/react-query'
import { useLocation } from 'react-router'
import { MapShell } from '@/layouts/MapShell'
import { markerListSchema, type Marker } from '@/shared/api/markers'
import { ApiError } from '@/shared/api/ApiError'
import { publicKeys } from '@/shared/query/keys'
import { usePlaceBrowse, type PlaceReads } from '@/features/places/usePlaceBrowse'
import { clusterPlaces } from '@/features/map/clusters'
import { distanceMeters } from '@/features/places/model'
import '@/features/places/places.css'
import { syntheticPlace } from './placeFixtures'

type Measurement = Record<string, unknown>
const twoFrames = () =>
    new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
    )
const heap = () =>
    (performance as Performance & { memory?: { usedJSHeapSize: number } }).memory?.usedJSHeapSize ??
    null

/** Uses the product controller/components with an isolated local synthetic read adapter. */
export default function PlacesPerformancePage() {
    const [client] = useState(() => new QueryClient())
    useEffect(() => () => client.clear(), [client])
    return (
        <QueryClientProvider client={client}>
            <PerformanceRun />
        </QueryClientProvider>
    )
}
function PerformanceRun() {
    const client = useQueryClient(),
        route = useLocation()
    const design = new URLSearchParams(route.search).get('design')
    const records = useRef<Marker[]>(
        design
            ? [
                  syntheticPlace({
                      title:
                          design === 'long'
                              ? '无障碍卫生间与母婴设施位置说明'.repeat(12)
                              : '600 South Wanping Road, Shanghai, Accessible Toilet',
                      description:
                          design === 'long'
                              ? '这是一段用于本机验收的长描述，检查手机滚动与分享导航按钮可达性。'.repeat(
                                    20,
                                )
                              : "Sip handcrafted cocktails and enjoy waterfront views at NYC's favorite floating restaurant.",
                      openTimeStart: '09:00',
                      openTimeEnd: '21:00',
                      markImage:
                          design === 'desktop'
                              ? '/uploads/markers/__dev_figma_desktop.png'
                              : '/uploads/markers/__dev_figma_mobile.jpg',
                  }),
              ]
            : [],
    )
    const [busy, setBusy] = useState(false)
    const [report, setReport] = useState<Measurement>({
        note: 'DEV-only synthetic frontend test. Load, interact, then record a snapshot.',
    })
    const measurements = useRef<Measurement[]>([])
    const interactions = useRef<Measurement[]>([])
    const frames = useRef({ max: 0, last: 0, count: 0 })
    const longTasks = useRef<number[]>([])
    const measured = useRef(false)
    const reads = useMemo<PlaceReads>(
        () => ({
            readViewport: async () => records.current,
            readSearch: async (query) =>
                records.current.filter((place) =>
                    place.title.toLowerCase().includes(query.toLowerCase()),
                ),
            readNearby: async (filters) =>
                records.current.filter(
                    (place) =>
                        place.category === filters.category &&
                        distanceMeters(place, filters) <= filters.radius,
                ),
            readPublicPlace: async (id) => {
                const place = records.current.find((item) => String(item.id) === id)
                if (!place) throw new ApiError(404, 'Not found')
                return place
            },
        }),
        [],
    )
    const id = new URLSearchParams(route.search).get('markerId')
    const browse = usePlaceBrowse('en', id, 'Synthetic', reads)
    useEffect(() => {
        let frame = 0
        const tick = (time: number) => {
            if (measured.current && document.visibilityState === 'visible') {
                if (frames.current.last)
                    frames.current.max = Math.max(frames.current.max, time - frames.current.last)
                frames.current.last = time
                frames.current.count++
            } else frames.current.last = 0
            frame = requestAnimationFrame(tick)
        }
        frame = requestAnimationFrame(tick)
        const observer = PerformanceObserver.supportedEntryTypes.includes('longtask')
            ? new PerformanceObserver((list) => {
                  if (measured.current)
                      longTasks.current.push(...list.getEntries().map((entry) => entry.duration))
              })
            : null
        observer?.observe({ type: 'longtask' })
        return () => {
            cancelAnimationFrame(frame)
            observer?.disconnect()
        }
    }, [])
    const snapshot = () => {
        measured.current = false
        const map = document.querySelector<HTMLElement>('.product-map')
        setReport({
            environment: {
                userAgent: navigator.userAgent,
                viewport: [innerWidth, innerHeight],
                mode: 'Vite development; synthetic local HTTP; OSM tiles excluded from readiness',
                visibility: document.visibilityState,
            },
            samples: measurements.current,
            interactions: interactions.current,
            frames: { maximumIntervalMs: frames.current.max, count: frames.current.count },
            longTasks: PerformanceObserver.supportedEntryTypes.includes('longtask')
                ? longTasks.current
                : null,
            usedHeapBytes: heap(),
            mapInstance: map?.dataset.mapId,
            zoom: map?.dataset.zoom,
            markerDomNodes: document.querySelectorAll('.leaflet-marker-icon').length,
            renderedRows: document.querySelectorAll('.place-result-row').length,
            totalRecords: records.current.length,
        })
    }
    const load = async (count: number) => {
        setBusy(true)
        frames.current = { max: 0, last: 0, count: 0 }
        longTasks.current = []
        interactions.current = []
        measured.current = true
        const before = heap(),
            start = performance.now()
        try {
            const response = await fetch(`/__dev/fixtures/places?count=${count}`, {
                cache: 'no-store',
                credentials: 'omit',
            })
            const body = await response.text(),
                received = performance.now()
            if (!response.ok) throw new Error(`Fixture HTTP ${response.status}`)
            const parsed: unknown = JSON.parse(body),
                parsedAt = performance.now()
            const data = markerListSchema.parse(parsed),
                validatedAt = performance.now()
            const index = clusterPlaces(data),
                clusteredAt = performance.now()
            const clusters = index.getClusters([-180, -90, 180, 90], 11),
                queriedAt = performance.now()
            const sample: Measurement = {
                count,
                rawBytes: new TextEncoder().encode(body).byteLength,
                httpBodyMs: received - start,
                jsonParseMs: parsedAt - received,
                validationMs: validatedAt - parsedAt,
                geoJsonAndIndexMs: clusteredAt - validatedAt,
                clusterQueryMs: queriedAt - clusteredAt,
                clustersAtZoom11: clusters.length,
                heapBeforeBytes: before,
            }
            const renderStart = performance.now()
            records.current = data
            await client.invalidateQueries({ queryKey: publicKeys.all })
            browse.focusPoint({ lat: 31.2304, lng: 121.4737 }, 11)
            await twoFrames()
            sample.updateToTwoFramesMs = performance.now() - renderStart
            sample.heapAfterBytes = heap()
            sample.mapInstance = document.querySelector<HTMLElement>('.product-map')?.dataset.mapId
            measurements.current.push(sample)
            setReport({
                latest: sample,
                note: 'Interact with the map and panels, then record a snapshot. Two frames is not a GPU paint-completion measurement.',
            })
        } catch (error) {
            setReport({ error: error instanceof Error ? error.message : String(error) })
        } finally {
            setBusy(false)
        }
    }
    if (design) return <MapShell browse={browse} />
    return (
        <div
            onClickCapture={(event) => {
                if (!measured.current || (event.target as Element).closest('.performance-controls'))
                    return
                const target = (event.target as Element).closest('button, a')
                if (!target) return
                const start = performance.now(),
                    label = target.getAttribute('aria-label') ?? target.textContent
                void twoFrames().then(() =>
                    interactions.current.push({
                        action: label,
                        toTwoFramesMs: performance.now() - start,
                        mapInstance:
                            document.querySelector<HTMLElement>('.product-map')?.dataset.mapId,
                    }),
                )
            }}
        >
            <MapShell browse={browse} />
            <aside
                className="performance-controls"
                style={{
                    position: 'fixed',
                    zIndex: 100,
                    right: 80,
                    top: 120,
                    width: 330,
                    maxHeight: 330,
                    overflow: 'auto',
                    background: 'white',
                    color: '#222',
                    padding: 12,
                    border: '1px solid #888',
                    font: '12px/1.4 var(--ui-font-body)',
                }}
            >
                <strong>DEV synthetic performance</strong>
                <br />
                <button disabled={busy} onClick={() => void load(1000)}>
                    Load 1,000
                </button>{' '}
                <button disabled={busy} onClick={() => void load(10000)}>
                    Load 10,000
                </button>{' '}
                <button disabled={busy} onClick={snapshot}>
                    Record snapshot
                </button>
                <pre data-testid="performance-report" style={{ whiteSpace: 'pre-wrap' }}>
                    {JSON.stringify(report, null, 2)}
                </pre>
            </aside>
        </div>
    )
}
