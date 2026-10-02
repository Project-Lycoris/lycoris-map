import { ApiError } from './ApiError'
import { ZodError } from 'zod'
/** Display only actionable categories, never an HTML proxy error or internal stack. */
export function readError(error: unknown): string | null {
    if (
        !error ||
        ((error instanceof Error || error instanceof DOMException) && error.name === 'AbortError')
    )
        return null
    if (error instanceof ApiError) {
        if (error.kind === 'timeout' || error.status === 408 || error.status === 504)
            return 'Loading places timed out. Try again.'
        if (error.status === 0) return 'Network unavailable. Check your connection and try again.'
        if (error.status === 401) return 'Your session expired. Please log in again.'
        if (error.status === 403) return 'You do not have permission to view these places.'
        if (error.status === 404) return 'This place is unavailable.'
        if (error.status === 429) return 'Too many requests. Please wait before trying again.'
        if (error.status >= 500)
            return 'The place service is temporarily unavailable. Try again later.'
    }
    if (error instanceof ZodError || error instanceof SyntaxError)
        return 'The place data could not be read. Try again.'
    return 'Could not load places. Try again.'
}
