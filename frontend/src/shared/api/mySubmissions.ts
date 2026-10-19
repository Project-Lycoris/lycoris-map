import { z } from 'zod'
import { request } from './transport'
import { ApiError } from './ApiError'
import type { Language } from '@/shared/i18n'

const id = z.number().int().positive().safe()
export const submissionSchema = z.object({
    kind: z.enum(['marker', 'edit', 'image']),
    id,
    markerId: id.nullable(),
    markerTitle: z.string(),
    status: z.enum(['PENDING', 'APPROVED', 'REJECTED']),
    reviewNote: z.string().nullable(),
    createdAt: z.string(),
    reviewedAt: z.string().nullable(),
    imageUrl: z.string().optional(),
})
export type Submission = z.infer<typeof submissionSchema>

/**
 * The contributor's own submissions. A 404 means the backend has not shipped
 * the route yet, which is not an error for the profile: the whole block hides.
 */
export async function readMySubmissions(
    language: Language,
    signal: AbortSignal,
): Promise<Submission[] | null> {
    try {
        return z.array(submissionSchema).parse(
            await request('/api/markers/me/submissions', {
                query: { lang: language },
                signal,
                cache: 'no-store',
            }),
        )
    } catch (error) {
        if (error instanceof ApiError && error.status === 404) return null
        throw error
    }
}
