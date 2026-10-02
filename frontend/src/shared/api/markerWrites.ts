import { z } from 'zod'
import { markerCategorySchema, markerSchema, venueTypeSchema } from './markers'
import { request } from './transport'

const time = z.string().regex(/^(?:[01]\d|2[0-3]):[0-5]\d$|^$/)
export const markerTextSchema = z
    .object({
        title: z.string().refine((value) => value.trim().length > 0 && [...value].length <= 120),
        category: markerCategorySchema,
        categories: z.array(markerCategorySchema).min(1).max(4).optional(),
        openingHoursNote: z.string().max(1000).optional(),
        venueType: venueTypeSchema.nullable().optional(),
        description: z.string(),
        language: z.enum(['en', 'zh']),
        isPublic: z.boolean(),
        openTimeStart: time,
        openTimeEnd: time,
    })
    .refine((value) => Boolean(value.openTimeStart) === Boolean(value.openTimeEnd))
    .refine(
        (value) =>
            (value.categories ?? [value.category]).includes('accessible_toilet') ||
            value.venueType == null,
    )
export const markerCreateSchema = markerTextSchema.safeExtend({
    lat: z.number().min(-90).max(90),
    lng: z.number().min(-180).max(180),
    clientRequestId: z.uuid(),
})
export type MarkerText = z.infer<typeof markerTextSchema>
export type MarkerCreate = z.infer<typeof markerCreateSchema>

export async function createMarker(value: MarkerCreate, signal?: AbortSignal) {
    const payload = markerCreateSchema.parse(value)
    return markerSchema.parse(
        await request('/api/markers', {
            method: 'POST',
            json: payload,
            query: { lang: payload.language },
            ...(signal ? { signal } : {}),
        }),
    )
}

/** A successful PATCH returns the unchanged marker; the edit awaits review. */
export async function proposeMarkerEdit(id: number, value: MarkerText, signal?: AbortSignal) {
    const payload = markerTextSchema.parse(value)
    return markerSchema.parse(
        await request(`/api/markers/${z.number().int().positive().safe().parse(id)}`, {
            method: 'PATCH',
            json: payload,
            query: { lang: payload.language },
            ...(signal ? { signal } : {}),
        }),
    )
}
