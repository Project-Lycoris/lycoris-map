/**
 * Public DTO schemas for the marker read endpoints, modelled on
 * `backend/src/modules/markers/model.rs` (`MarkerDto`, including soft-deletion state).
 *
 * Only the read shape used by S1/S3 lives here; write DTOs arrive with S5.
 * `id`/`version` are Rust `i64`, so they are validated as safe integers rather
 * than allowed to silently lose precision.
 */
import { z } from 'zod'
import { MARKER_CATEGORIES } from '@/shared/query/keys'

const safeInteger = z.number().int().safe()

export const markerCategorySchema = z.enum(MARKER_CATEGORIES)

export const venueTypeSchema = z.enum([
    'metro',
    'hospital',
    'mall',
    'railway_station',
    'school',
    'public_toilet',
    'airport',
    'park',
    'other',
])
export type VenueType = z.infer<typeof venueTypeSchema>
// Read future string values without dropping the entire marker list. Omitted
// tags remain null when editing, so a newer server's classification is preserved.
export const venueTypeReadSchema = z
    .string()
    .transform((value) => venueTypeSchema.safeParse(value).data ?? null)
    .nullable()
    .optional()

export const reviewStatusSchema = z.enum(['PENDING', 'APPROVED', 'REJECTED'])

/** `HH:mm`, or the backend's `00:00` all-day marker. */
export const openTimeSchema = z.string()

export const markerSchema = z.object({
    id: safeInteger,
    version: safeInteger,
    lat: z.number().min(-90).max(90),
    lng: z.number().min(-180).max(180),
    category: markerCategorySchema,
    categories: z.array(markerCategorySchema).optional(),
    openingHoursNote: z.string().nullable().optional(),
    photos: z
        .array(z.object({ id: safeInteger, url: z.string(), sortOrder: safeInteger }))
        .optional(),
    venueType: venueTypeReadSchema,
    hoursTimezone: z.string().optional(),
    title: z.string(),
    description: z.string().nullable(),
    sourceLanguage: z.string(),
    contentLanguage: z.string(),
    isPublic: z.boolean(),
    username: z.string(),
    userPublicId: z.string().nullable(),
    clientRequestId: z.string().nullable(),
    isActive: z.boolean(),
    // Optional while older production responses and native clients transition.
    deactivated: z.boolean().optional(),
    openTimeStart: openTimeSchema.nullable(),
    openTimeEnd: openTimeSchema.nullable(),
    reviewStatus: reviewStatusSchema,
    lastEditedBy: z.string().nullable(),
    lastEditedByPublicId: z.string().nullable(),
    lastEditedByOwner: z.boolean(),
    markImage: z.string().nullable(),
    createdAt: z.string(),
    updatedAt: z.string(),
})

export type Marker = z.infer<typeof markerSchema>

/** `/api/markers/{viewport,nearby,search}` answer with a bare array. */
export const markerListSchema = z.array(markerSchema)

/** `/api/markers/me/favorites` answers with an array of marker IDs. */
export const markerIdListSchema = z.array(safeInteger)
