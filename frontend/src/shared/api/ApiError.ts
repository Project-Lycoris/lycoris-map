/**
 * Transport error shape shared by every endpoint parser.
 *
 * The Rust backend uses four response body shapes (ApiResponse envelope,
 * plain JSON, plain text, empty body) and does NOT wrap everything in a generic
 * envelope, so this error is built by the transport from status + body shape
 * instead of from a single expected JSON structure.
 */
export class ApiError extends Error {
    readonly kind: 'http' | 'network' | 'timeout'
    readonly status: number
    readonly code: number | undefined
    readonly requestId: string | undefined
    readonly body: string | undefined
    readonly accessDenied: boolean

    constructor(
        status: number,
        message: string,
        options: {
            kind?: 'http' | 'network' | 'timeout'
            code?: number | undefined
            requestId?: string | undefined
            body?: string
            accessDenied?: boolean
        } = {},
    ) {
        super(message)
        this.name = 'ApiError'
        this.kind = options.kind ?? (status === 0 ? 'network' : 'http')
        this.status = status
        this.code = options.code
        this.requestId = options.requestId
        this.body = options.body
        this.accessDenied = options.accessDenied === true
    }

    /** Network/timeout failure: no HTTP response was received. */
    static network(
        message: string,
        requestId?: string | undefined,
        kind: 'network' | 'timeout' = 'network',
    ): ApiError {
        return new ApiError(0, message, { requestId, kind })
    }
}

export function isApiError(value: unknown): value is ApiError {
    return value instanceof ApiError
}
