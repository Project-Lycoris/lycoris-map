import type { Marker, VenueType } from '@/shared/api/markers'
import type { MarkerText } from '@/shared/api/markerWrites'
import type { Language } from '@/shared/query/keys'
import type { LatLng } from '@/features/map/coords'

export type ContributionDraft = {
    title: string
    category: 'toilet' | 'nursing' | 'medical' | 'custom' | null
    categories?: Array<'toilet' | 'nursing' | 'medical' | 'custom'>
    openingHoursNote?: string
    photos?: File[]
    venueType: VenueType | null
    description: string
    openingHour: string
    openingMinute: string
    closingHour: string
    closingMinute: string
    isPublic: boolean
    photo: File | null
}
export const emptyContributionDraft: ContributionDraft = {
    title: '',
    category: null,
    venueType: 'other',
    categories: [],
    openingHoursNote: '',
    photos: [],
    description: '',
    openingHour: '',
    openingMinute: '',
    closingHour: '',
    closingMinute: '',
    isPublic: true,
    photo: null,
}
export const categories = {
    toilet: 'accessible_toilet',
    nursing: 'baby_room',
    medical: 'friendly_clinic',
    custom: 'self_definition',
} as const
export function draftFromMarker(marker: Marker): ContributionDraft {
    const category = (Object.keys(categories) as (keyof typeof categories)[]).find(
        (key) => categories[key] === marker.category,
    )!
    const start = marker.openTimeStart?.slice(0, 5).split(':') ?? []
    const end = marker.openTimeEnd?.slice(0, 5).split(':') ?? []
    return {
        ...emptyContributionDraft,
        category,
        categories: (marker.categories ?? [marker.category]).flatMap((value) => {
            const key = (Object.keys(categories) as (keyof typeof categories)[]).find(
                (key) => categories[key] === value,
            )
            return key ? [key] : []
        }),
        openingHoursNote: marker.openingHoursNote ?? '',
        venueType: marker.venueType ?? null,
        title: marker.title,
        description: marker.description ?? '',
        isPublic: marker.isPublic,
        openingHour: start[0] ?? '',
        openingMinute: start[1] ?? '',
        closingHour: end[0] ?? '',
        closingMinute: end[1] ?? '',
    }
}
export function draftText(draft: ContributionDraft, _language: Language): MarkerText {
    const title = draft.title.trim()
    if (!title || [...title].length > 120) throw new Error('Enter a title of 1–120 characters.')
    const selected = draftCategories(draft)
    if (!selected.length) throw new Error('Choose a category.')
    const values = [draft.openingHour, draft.openingMinute, draft.closingHour, draft.closingMinute]
    if (
        values.some(Boolean) &&
        !values.every(
            (value, index) =>
                /^\d{1,2}$/.test(value) && Number(value) <= (index % 2 === 0 ? 23 : 59),
        )
    )
        throw new Error('Complete both opening and closing times, or leave all four fields empty.')
    return {
        title,
        category: categories[selected[0]!],
        ...(draft.categories?.length
            ? { categories: selected.map((value) => categories[value]) }
            : {}),
        ...(draft.openingHoursNote !== undefined
            ? { openingHoursNote: draft.openingHoursNote }
            : {}),
        venueType: selected.includes('toilet') ? draft.venueType : null,
        description: draft.description,
        language: 'zh',
        isPublic: draft.isPublic,
        openTimeStart: values.every(Boolean)
            ? `${values[0]!.padStart(2, '0')}:${values[1]!.padStart(2, '0')}`
            : '',
        openTimeEnd: values.every(Boolean)
            ? `${values[2]!.padStart(2, '0')}:${values[3]!.padStart(2, '0')}`
            : '',
    }
}
export function checkPoint(point: LatLng | null): asserts point is LatLng {
    if (
        !point ||
        !Number.isFinite(point.lat) ||
        !Number.isFinite(point.lng) ||
        Math.abs(point.lat) > 90 ||
        Math.abs(point.lng) > 180
    )
        throw new Error('Choose the place location on the map before submitting.')
}

export function draftCategories(draft: ContributionDraft): Array<keyof typeof categories> {
    return draft.categories?.length ? draft.categories : draft.category ? [draft.category] : []
}
export function draftPhotos(draft: ContributionDraft): File[] {
    return draft.photos?.length ? draft.photos : draft.photo ? [draft.photo] : []
}
