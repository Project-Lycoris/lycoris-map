import type { Marker } from '@/shared/api/markers'
import { categoryLabels } from '@/features/places/model'
import { venueLabels } from '@/features/places/venue'
import type { VenueType } from '@/shared/api/markers'
import type { EditProposal } from './api'

export type ComparisonRow = {
    key: string
    label: string
    current: string
    proposed: string
    changed: boolean
}

const EMPTY = '—'

function categories(values: (keyof typeof categoryLabels)[], t: (value: string) => string): string {
    return values.length ? values.map((value) => t(categoryLabels[value])).join(' · ') : EMPTY
}

function venue(value: VenueType | null | undefined, t: (value: string) => string): string {
    return value ? t(venueLabels[value]) : EMPTY
}

export function comparisonRows(
    current: Marker | undefined,
    edit: EditProposal,
    t: (value: string) => string,
): ComparisonRow[] {
    const proposedCategories = edit.categories ?? [edit.category]
    const currentCategories = current ? (current.categories ?? [current.category]) : []
    const rows: Omit<ComparisonRow, 'changed'>[] = [
        {
            key: 'title',
            label: 'Title',
            current: current?.title ?? EMPTY,
            proposed: edit.title,
        },
        {
            key: 'categories',
            label: 'Category',
            current: categories(currentCategories, t),
            proposed: categories(proposedCategories, t),
        },
        {
            key: 'venue',
            label: 'Venue type',
            current: venue(current?.venueType, t),
            proposed: venue(edit.venueType, t),
        },
        {
            key: 'description',
            label: 'Description',
            current: current?.description || EMPTY,
            proposed: edit.description || EMPTY,
        },
        {
            key: 'openStart',
            label: 'Opening time',
            current: current?.openTimeStart || EMPTY,
            proposed: edit.openTimeStart || EMPTY,
        },
        {
            key: 'openEnd',
            label: 'Closing time',
            current: current?.openTimeEnd || EMPTY,
            proposed: edit.openTimeEnd || EMPTY,
        },
        {
            key: 'hoursNote',
            label: 'Opening hours note',
            current: current?.openingHoursNote || EMPTY,
            proposed: edit.openingHoursNote || EMPTY,
        },
        {
            key: 'public',
            label: 'Public',
            current: current ? t(current.isPublic ? 'Yes' : 'No') : EMPTY,
            proposed: t(edit.isPublic ? 'Yes' : 'No'),
        },
        {
            key: 'active',
            label: 'Active flag',
            current: current ? t(current.isActive ? 'Yes' : 'No') : EMPTY,
            proposed: t(edit.isActive ? 'Yes' : 'No'),
        },
        {
            key: 'language',
            label: 'Language',
            current: current?.contentLanguage || EMPTY,
            proposed: edit.language,
        },
    ]
    return rows.map((row) => ({ ...row, changed: row.current !== row.proposed }))
}
