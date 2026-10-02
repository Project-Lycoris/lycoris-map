import { readError } from '@/shared/api/readError'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import type { Marker } from '@/shared/api/markers'
import { ApiError } from '@/shared/api/ApiError'
import {
    parseMarkerId,
    readNearby,
    readPublicPlace,
    readSearch,
    readViewport,
} from '@/shared/api/markerReads'
import { publicKeys, privateKeys, type Language, type MarkerCategory } from '@/shared/query/keys'
import { useSession } from '@/features/auth/SessionProvider'
import { readAccountPlace } from '@/shared/api/privatePlaces'
import type { LatLng } from '@/features/map/coords'
import type { MapFocus, MapView } from '@/features/map/viewport'
import { viewportWindow, type ViewportWindow } from '@/features/map/viewportWindow'
import { usePreferences } from '@/features/preferences/PreferencesProvider'
import { useLocationFix } from '@/features/map/useLocationFix'

function useDebounced<T>(value: T, delay: number): T {
    const [settled, setSettled] = useState(value)
    useEffect(() => {
        const timer = setTimeout(() => setSettled(value), delay)
        return () => clearTimeout(timer)
    }, [value, delay])
    return settled
}
const retry = (count: number, error: Error) =>
    count < 1 && (!(error instanceof ApiError) || error.status === 0 || error.status >= 500)
const readOptions = {
    retry,
    staleTime: 30_000,
    refetchInterval: 60_000,
    refetchOnWindowFocus: true,
}
export type PlaceReadState = {
    pending: boolean
    error: string | null
    retry: () => void
    retryable?: boolean
}

export type PlaceReads = Pick<
    typeof import('@/shared/api/markerReads'),
    'readViewport' | 'readNearby' | 'readSearch' | 'readPublicPlace'
>
const defaultReads: PlaceReads = { readViewport, readNearby, readSearch, readPublicPlace }

export function usePlaceBrowse(
    language: Language,
    rawMarkerId: string | null,
    initialSearch = '',
    reads: PlaceReads = defaultReads,
) {
    const { preferences } = usePreferences()
    const client = useQueryClient()
    const session = useSession()
    const scope = reads === defaultReads ? session.scope : null
    const [view, setView] = useState<MapView | null>(null)
    const settledView = useDebounced(view, 250)
    const [viewport, setViewport] = useState<ViewportWindow | null>(null)
    useEffect(() => {
        if (settledView) setViewport((previous) => viewportWindow(previous, settledView))
    }, [settledView])
    const [search, setSearchState] = useState(initialSearch)
    const [nearby, setNearby] = useState<{
        point: LatLng
        category: MarkerCategory
        located: boolean
    } | null>(null)
    const [clusterIds, setClusterIds] = useState<readonly number[] | null>(null)
    const term = search.trim()
    const query = useDebounced(term, 300)
    const [focus, setFocus] = useState<MapFocus | null>(null)
    const focusSequence = useRef(0)
    const focusPoint = useCallback(
        (point: LatLng, zoom = 15) =>
            setFocus({ point, key: `focus-${++focusSequence.current}`, zoom }),
        [],
    )
    const onLocated = useCallback(
        (point: LatLng, automatic: boolean) => {
            // A late initial permission grant must not replace an opened/shared place.
            if (!automatic || (!rawMarkerId && focusSequence.current === 0)) focusPoint(point)
            setNearby((previous) => (previous ? { ...previous, point, located: true } : null))
        },
        [focusPoint, rawMarkerId],
    )
    const location = useLocationFix(onLocated)
    const listPositions = useRef(
        new Map<
            string,
            { offset: number; active: number; sizes?: Map<number, number>; width?: number }
        >(),
    )
    const onView = useCallback(
        (next: MapView) =>
            setView((previous) =>
                JSON.stringify(previous) === JSON.stringify(next) ? previous : next,
            ),
        [],
    )
    const setSearch = (value: string) => {
        setSearchState(value)
        setNearby(null)
        setClusterIds(null)
    }
    const chooseCategory = (category: MarkerCategory) => {
        const point = location.position ?? view?.center
        if (!point) return
        setSearchState('')
        setClusterIds(null)
        setNearby({ point, category, located: location.position !== null })
    }
    const mapQuery = useQuery<Marker[]>({
        ...readOptions,
        queryKey: [
            ...publicKeys.markers(),
            'viewport-set',
            language,
            viewport?.scale ?? null,
            viewport?.bounds ?? null,
        ],
        enabled: viewport !== null,
        // Keep existing pins during a region update, but never show an old
        // language's DTOs while loading another language.
        placeholderData: (previous, query) =>
            query?.queryKey[3] === language ? previous : undefined,
        queryFn: async ({ signal }) => {
            const batches = await Promise.all(
                viewport!.bounds.map((bounds) =>
                    reads.readViewport({ ...bounds, categories: [] }, language, signal),
                ),
            )
            return [...new Map(batches.flat().map((marker) => [marker.id, marker])).values()]
        },
    })
    // `placeholderData` only survives while a query is pending; React Query
    // clears `data` when the latest window settles in error. Keep the last
    // successful public viewport payload per language so a failed/cancelled
    // region read cannot blank every already-valid pin. A successful empty
    // response overwrites this (honest empty), and a language change isolates
    // it immediately. Only the public viewport write path feeds this ref, so
    // owner-private detail data can never leak into public markers.
    const lastMapData = useRef<{ language: Language; markers: readonly Marker[] } | null>(null)
    useEffect(() => {
        // A placeholder is the previous window's data marked as success; it is
        // not a freshly confirmed result and must not overwrite the retained
        // fallback (which would also defeat the 404 pruning below).
        if (!mapQuery.isSuccess || mapQuery.isPlaceholderData) return
        lastMapData.current = { language, markers: mapQuery.data }
    }, [
        mapQuery.isSuccess,
        mapQuery.isPlaceholderData,
        mapQuery.dataUpdatedAt,
        mapQuery.data,
        language,
    ])
    const nearbyFilters = nearby
        ? {
              lat: Math.round(nearby.point.lat * 1e6) / 1e6,
              lng: Math.round(nearby.point.lng * 1e6) / 1e6,
              radius: preferences.radius,
              category: nearby.category,
          }
        : null
    const nearbyQuery = useQuery({
        ...readOptions,
        queryKey: nearbyFilters
            ? publicKeys.nearby(language, nearbyFilters)
            : ['public', 'nearby-idle'],
        enabled: nearbyFilters !== null && !term,
        queryFn: ({ signal }) => reads.readNearby(nearbyFilters!, language, signal),
    })
    const searchQuery = useQuery({
        ...readOptions,
        queryKey: publicKeys.search(language, { query }),
        enabled: !!query && query === term,
        queryFn: ({ signal }) => reads.readSearch(query, language, signal),
    })
    const id = parseMarkerId(rawMarkerId)
    const detailQuery = useQuery({
        ...readOptions,
        queryKey: scope
            ? privateKeys.detail(scope, language, id ?? '')
            : publicKeys.anonymousDetail(language, id ?? ''),
        enabled:
            id !== null && (!session.store || (!session.busy && session.status !== 'checking')),
        queryFn: ({ signal }) =>
            scope && session.store
                ? session.store.runPrivate(scope, (s) => readAccountPlace(id!, language, s), signal)
                : reads.readPublicPlace(id!, language, signal),
    })
    const unavailable = detailQuery.error instanceof ApiError && detailQuery.error.status === 404
    useEffect(() => {
        if (!id || !unavailable) return
        const lists = {
            predicate: (query: { queryKey: readonly unknown[] }) =>
                query.queryKey[0] === 'public' &&
                query.queryKey[1] === 'markers' &&
                query.queryKey[2] !== 'detail',
        }
        // Cancel pre-404 responses, remove the stale item from caches and from
        // the retained fallback, then allow fresh public reads to restore it if
        // it becomes visible again. Pruning the ref matters because a later
        // viewport failure falls back to it even after the detail is closed.
        void client.cancelQueries(lists).then(() => {
            client.setQueriesData<Marker[]>(lists, (data) =>
                data?.filter((marker) => String(marker.id) !== id),
            )
            if (lastMapData.current?.language === language)
                lastMapData.current = {
                    language,
                    markers: lastMapData.current.markers.filter(
                        (marker) => String(marker.id) !== id,
                    ),
                }
            void client.invalidateQueries(lists)
        })
    }, [client, id, unavailable, language])
    const activeQuery = term ? searchQuery : nearby ? nearbyQuery : mapQuery
    const debouncing = !!term && term !== query
    const fallback =
        !term && !nearby && lastMapData.current?.language === language
            ? lastMapData.current.markers
            : undefined
    const allResults: readonly Marker[] = useMemo(
        () =>
            debouncing
                ? []
                : (activeQuery.data ?? fallback ?? []).filter(
                      (marker) => !(unavailable && String(marker.id) === id),
                  ),
        [debouncing, activeQuery.data, fallback, unavailable, id],
    )
    const results = useMemo(() => {
        if (!clusterIds) return allResults
        const ids = new Set(clusterIds)
        return allResults.filter((marker) => ids.has(marker.id))
    }, [allResults, clusterIds])
    const mode = clusterIds ? 'cluster' : term ? 'search' : nearby ? 'nearby' : 'map'
    const state: PlaceReadState = {
        pending: debouncing || activeQuery.isPending,
        error: debouncing ? null : readError(activeQuery.error),
        retry: () => {
            void activeQuery.refetch()
        },
    }
    const detailState: PlaceReadState = {
        retryable: id !== null,
        pending: id !== null && detailQuery.isPending,
        error:
            rawMarkerId !== null && id === null
                ? 'This place link is invalid.'
                : readError(detailQuery.error),
        retry: () => {
            if (id) void detailQuery.refetch()
        },
    }
    return {
        language,
        view,
        onView,
        search,
        setSearch,
        mode,
        nearby: nearby ? { ...nearby, radius: preferences.radius } : null,
        chooseCategory,
        results,
        markers: results,
        state,
        detail: unavailable ? undefined : detailQuery.data,
        detailState,
        selectedId: id,
        focus,
        focusPoint,
        location,
        listPositions,
        clusterIds,
        showCluster: (ids: number[] | null) => {
            setClusterIds(ids)
        },
        clearResults: () => {
            setSearchState('')
            setNearby(null)
            setClusterIds(null)
        },
    }
}
export type PlaceBrowse = ReturnType<typeof usePlaceBrowse>
