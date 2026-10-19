import type { MarkerText } from '@/shared/api/markerWrites'
import type { EditProposal, ImageProposal, ReviewMarker, SubmissionStats } from './api'

export type ReviewKind = 'marker' | 'edit' | 'image'

type Base = {
    key: string
    id: number
    title: string
    createdAt: string
    submitter: string
    stats?: SubmissionStats | undefined
}

export type ReviewItem =
    | (Base & { kind: 'marker'; marker: ReviewMarker })
    | (Base & { kind: 'edit'; markerId: number; edit: EditProposal })
    | (Base & { kind: 'image'; markerId: number; image: ImageProposal })

export function markerItem(marker: ReviewMarker): ReviewItem {
    return {
        kind: 'marker',
        key: `marker-${marker.id}`,
        id: marker.id,
        title: marker.title,
        createdAt: marker.createdAt,
        submitter: marker.username,
        stats: marker.proposerSubmissionStats,
        marker,
    }
}

export function editItem(edit: EditProposal): ReviewItem {
    return {
        kind: 'edit',
        key: `edit-${edit.id}`,
        id: edit.id,
        markerId: edit.markerId,
        title: edit.markerTitle,
        createdAt: edit.createdAt,
        submitter: edit.proposerUsername,
        stats: edit.proposerSubmissionStats,
        edit,
    }
}

export function imageItem(image: ImageProposal): ReviewItem {
    return {
        kind: 'image',
        key: `image-${image.id}`,
        id: image.id,
        markerId: image.markerId,
        title: image.markerTitle,
        createdAt: image.createdAt,
        submitter: image.proposerUsername,
        stats: image.proposerSubmissionStats,
        image,
    }
}

export function itemMarkerId(item: ReviewItem): number {
    return item.kind === 'marker' ? item.marker.id : item.markerId
}

export function itemMatches(item: ReviewItem, query: string): boolean {
    const needle = query.trim().toLowerCase()
    if (!needle) return true
    return [item.title, item.submitter, String(itemMarkerId(item)), `#${item.id}`, String(item.id)]
        .join(' ')
        .toLowerCase()
        .includes(needle)
}

export function kindLabel(kind: ReviewKind): string {
    return kind === 'marker'
        ? 'New place'
        : kind === 'edit'
          ? 'Content changes'
          : 'Photo submission'
}

export function proposalDraft(item: ReviewItem): MarkerText | null {
    if (item.kind === 'marker') return null
    if (item.kind === 'image') return null
    const edit = item.edit
    return {
        title: edit.title,
        category: edit.category,
        categories: edit.categories ?? [edit.category],
        openingHoursNote: edit.openingHoursNote ?? '',
        venueType: (edit.categories ?? [edit.category]).includes('accessible_toilet')
            ? (edit.venueType ?? null)
            : null,
        description: edit.description ?? '',
        isPublic: edit.isPublic,
        openTimeStart: edit.openTimeStart ?? '',
        openTimeEnd: edit.openTimeEnd ?? '',
        language: edit.language === 'en' ? 'en' : 'zh',
    }
}
