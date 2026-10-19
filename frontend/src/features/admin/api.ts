import { z } from 'zod'
import { ApiError } from '@/shared/api/ApiError'
import { request } from '@/shared/api/transport'
import {
    markerListSchema,
    markerSchema,
    markerCategorySchema,
    venueTypeReadSchema,
    reviewStatusSchema,
} from '@/shared/api/markers'
import { markerTextSchema, type MarkerText } from '@/shared/api/markerWrites'
import type { Language } from '@/shared/i18n'
const id = z.number().int().positive().safe()
const user = z.object({
    id,
    publicId: z.string(),
    username: z.string().nullable(),
    nickname: z.string().nullable(),
    email: z.string().nullable(),
    avatarUrl: z.string().nullable(),
    pronouns: z.string().nullable(),
    signature: z.string().nullable(),
    role: z.string(),
    deleted: z.boolean(),
    deletedAt: z.string().nullable(),
})
export const userPageSchema = z.object({
    page: z.number().int().nonnegative(),
    size: z.number().int().positive(),
    totalPages: z.number().int().nonnegative(),
    totalElements: z.number().int().nonnegative(),
    items: z.array(user),
})
export type AdminUser = z.infer<typeof user>
export const submissionStatsSchema = z.object({
    approved: z.number().int().nonnegative(),
    rejected: z.number().int().nonnegative(),
    pending: z.number().int().nonnegative(),
})
export type SubmissionStats = z.infer<typeof submissionStatsSchema>
export const reviewMarkerSchema = markerSchema.extend({
    proposerSubmissionStats: submissionStatsSchema.optional(),
})
export type ReviewMarker = z.infer<typeof reviewMarkerSchema>
export const editSchema = z.object({
    id,
    markerId: id,
    markerTitle: z.string(),
    lat: z.number(),
    lng: z.number(),
    category: markerCategorySchema,
    categories: z.array(markerCategorySchema).optional(),
    openingHoursNote: z.string().nullable().optional(),
    venueType: venueTypeReadSchema,
    title: z.string(),
    description: z.string().nullable(),
    language: z.string(),
    isPublic: z.boolean(),
    isActive: z.boolean(),
    openTimeStart: z.string().nullable(),
    openTimeEnd: z.string().nullable(),
    proposerUsername: z.string(),
    proposerPublicId: z.string().nullable(),
    proposerIsOwner: z.boolean(),
    status: reviewStatusSchema,
    baseMarkerVersion: z.number().int().safe().nullable().optional(),
    createdAt: z.string(),
    proposerSubmissionStats: submissionStatsSchema.optional(),
})
export const imageSchema = z.object({
    id,
    markerId: id,
    markerTitle: z.string(),
    proposerUsername: z.string(),
    proposerPublicId: z.string().nullable(),
    imageUrl: z.string(),
    status: reviewStatusSchema,
    createdAt: z.string(),
    proposerSubmissionStats: submissionStatsSchema.optional(),
})
export type EditProposal = z.infer<typeof editSchema>
export type ImageProposal = z.infer<typeof imageSchema>
const prefix = '/api/admin/markers'
export async function readUsers(page: number, q: string, signal: AbortSignal, size = 10) {
    return userPageSchema.parse(
        await request('/api/admin/users', { query: { page, size, q }, signal, cache: 'no-store' }),
    )
}

export async function readMarkers(all: boolean, language: Language, signal: AbortSignal) {
    return markerListSchema.parse(
        await request(`${prefix}/${all ? 'all' : 'pending'}`, {
            query: { lang: language },
            signal,
            cache: 'no-store',
        }),
    )
}
export async function readPendingMarkers(signal: AbortSignal) {
    return z.array(reviewMarkerSchema).parse(
        await request(`${prefix}/pending`, {
            query: { lang: 'zh' },
            signal,
            cache: 'no-store',
        }),
    )
}
export async function readEdits(signal: AbortSignal) {
    return z
        .array(editSchema)
        .parse(await request(`${prefix}/pending-edits`, { signal, cache: 'no-store' }))
}
export async function readImages(signal: AbortSignal) {
    return z
        .array(imageSchema)
        .parse(await request(`${prefix}/pending-images`, { signal, cache: 'no-store' }))
}
export type ModerationPayload = { changes?: MarkerText | undefined; reason?: string | undefined }
export function moderationPath(
    kind: 'markers' | 'edits' | 'images',
    value: number,
    decision: 'approve' | 'reject',
) {
    const path =
        kind === 'markers'
            ? `${id.parse(value)}`
            : `${kind === 'edits' ? 'edit' : 'image'}-proposals/${id.parse(value)}`
    return `${prefix}/${path}/${decision}`
}
export async function moderate(
    kind: 'markers' | 'edits' | 'images',
    value: number,
    decision: 'approve' | 'reject',
    language: Language,
    signal: AbortSignal,
    payload: ModerationPayload = {},
) {
    const body: ModerationPayload = {}
    // Images have no review body; only send fields the contract defines.
    if (kind !== 'images' && payload.changes !== undefined) body.changes = payload.changes
    if (payload.reason !== undefined && payload.reason.trim() !== '')
        body.reason = payload.reason.trim()
    await request(moderationPath(kind, value, decision), {
        method: 'POST',
        query: { lang: language },
        signal,
        cache: 'no-store',
        ...(Object.keys(body).length ? { json: body } : {}),
    })
}
export const batchKindSchema = z.enum(['marker', 'edit', 'image'])
export const batchDecisionSchema = z.enum(['approve', 'reject'])
export const batchResultSchema = z.object({
    results: z.array(
        z.object({
            kind: batchKindSchema,
            id,
            ok: z.boolean(),
            status: z.number().int().optional(),
            message: z.string().optional(),
        }),
    ),
})
export type BatchResult = z.infer<typeof batchResultSchema>['results'][number]
export type BatchRequestItem = {
    kind: z.infer<typeof batchKindSchema>
    id: number
    decision: z.infer<typeof batchDecisionSchema>
    reason?: string
}
export async function batchModerate(items: BatchRequestItem[], signal: AbortSignal) {
    return batchResultSchema.parse(
        await request('/api/admin/moderation/batch', {
            method: 'POST',
            json: { items },
            signal,
            cache: 'no-store',
        }),
    )
}

export async function editMarker(value: number, text: MarkerText, signal: AbortSignal) {
    const payload = markerTextSchema.parse(text)
    return markerSchema.parse(
        await request(`${prefix}/${id.parse(value)}`, {
            method: 'PATCH',
            json: payload,
            query: { lang: payload.language },
            signal,
            cache: 'no-store',
        }),
    )
}
export async function deactivateMarker(value: number, signal: AbortSignal) {
    await request(`${prefix}/${id.parse(value)}`, { method: 'DELETE', signal, cache: 'no-store' })
}
export async function restoreMarker(value: number, signal: AbortSignal) {
    await request(`${prefix}/${id.parse(value)}/restore`, {
        method: 'POST',
        signal,
        cache: 'no-store',
    })
}

export async function changeUser(
    value: number,
    action: 'disable' | 'restore' | 'reset-password',
    signal: AbortSignal,
) {
    return z
        .object({ message: z.string() })
        .parse(
            await request(
                `/api/admin/users/${id.parse(value)}${action === 'disable' ? '' : `/${action}`}`,
                { method: action === 'disable' ? 'DELETE' : 'POST', signal, cache: 'no-store' },
            ),
        )
}

const markerSearchStatusSchema = z.enum(['PENDING', 'APPROVED', 'REJECTED'])
export const adminMarkerSchema = markerSchema.extend({
    reviewNote: z.string().nullable().optional(),
})
export type AdminMarker = z.infer<typeof adminMarkerSchema>
export const adminMarkerPageSchema = z.object({
    page: z.number().int().nonnegative(),
    size: z.number().int().positive(),
    totalPages: z.number().int().nonnegative(),
    totalElements: z.number().int().nonnegative(),
    items: z.array(adminMarkerSchema),
})
export type AdminMarkerPage = z.infer<typeof adminMarkerPageSchema>
export type MarkerSearch = {
    page: number
    size?: number | undefined
    q?: string | undefined
    status?: z.infer<typeof markerSearchStatusSchema> | undefined
    category?: string | undefined
    deactivated?: boolean | undefined
    sort?: 'updated_desc' | 'created_desc' | 'created_asc' | undefined
    language: Language
}
export async function searchMarkers(search: MarkerSearch, signal: AbortSignal) {
    return adminMarkerPageSchema.parse(
        await request('/api/admin/markers/search', {
            query: {
                page: search.page,
                size: search.size ?? 20,
                q: search.q,
                status: search.status,
                category: search.category,
                deactivated: search.deactivated,
                sort: search.sort,
                lang: search.language,
            },
            signal,
            cache: 'no-store',
        }),
    )
}

export const moderationActionSchema = z.enum([
    'approve',
    'approve_with_changes',
    'reject',
    'edit',
    'deactivate',
    'restore',
    'user_disable',
    'user_restore',
    'user_reset_password',
])
export type ModerationAction = z.infer<typeof moderationActionSchema>
export const moderationEventSchema = z.object({
    id,
    createdAt: z.string(),
    actorUsername: z.string(),
    actorPublicId: z.string(),
    action: moderationActionSchema,
    targetKind: z.enum(['marker', 'edit', 'image', 'user']),
    targetId: id,
    markerId: id.nullable(),
    markerTitle: z.string().nullable().optional(),
    targetLabel: z.string().nullable().optional(),
    note: z.string().nullable(),
})
export type ModerationEvent = z.infer<typeof moderationEventSchema>
export const moderationEventPageSchema = z.object({
    page: z.number().int().nonnegative(),
    size: z.number().int().positive(),
    totalPages: z.number().int().nonnegative(),
    totalElements: z.number().int().nonnegative(),
    items: z.array(moderationEventSchema),
})
export async function readModerationEvents(
    params: { page: number; size?: number; markerId?: number },
    signal: AbortSignal,
) {
    return moderationEventPageSchema.parse(
        await request('/api/admin/moderation/events', {
            query: {
                page: params.page,
                size: params.size ?? 20,
                markerId: params.markerId,
            },
            signal,
            cache: 'no-store',
        }),
    )
}
/**
 * True while the backend has not shipped a new route yet. `/markers/search`
 * currently collides with `/markers/{id}` and answers 405 (method not allowed)
 * instead of 404, so both mean "server update required" here.
 */
export function isMissingRoute(error: unknown) {
    return error instanceof ApiError && (error.status === 404 || error.status === 405)
}

const actionLabels: Record<ModerationAction, string> = {
    approve: 'Approved',
    approve_with_changes: 'Approved with changes',
    reject: 'Rejected',
    edit: 'Edited',
    deactivate: 'Disabled',
    restore: 'Restored',
    user_disable: 'Account disabled',
    user_restore: 'Account restored',
    user_reset_password: 'Password reset',
}
export function moderationActionLabel(action: ModerationAction): string {
    return actionLabels[action]
}

const targetLabels: Record<ModerationEvent['targetKind'], string> = {
    marker: 'New place',
    edit: 'Content changes',
    image: 'Photo submission',
    user: 'Account',
}
export function moderationTargetLabel(kind: ModerationEvent['targetKind']): string {
    return targetLabels[kind]
}
