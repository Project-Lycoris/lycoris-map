import { z } from 'zod'
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
    createdAt: z.string(),
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
export async function moderate(
    kind: 'markers' | 'edits' | 'images',
    value: number,
    decision: 'approve' | 'reject',
    language: Language,
    signal: AbortSignal,
) {
    const path =
        kind === 'markers'
            ? `${id.parse(value)}`
            : `${kind === 'edits' ? 'edit' : 'image'}-proposals/${id.parse(value)}`
    await request(`${prefix}/${path}/${decision}`, {
        method: 'POST',
        query: { lang: language },
        signal,
        cache: 'no-store',
    })
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
