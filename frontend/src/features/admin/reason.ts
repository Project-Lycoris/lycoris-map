/** Shortcut reasons are stored as English keys so every reader can translate. */
export const QUICK_REASONS = [
    'Information is inaccurate',
    'Duplicate place',
    'Wrong location',
    'Inappropriate photo',
    'Unrelated to accessibility or friendly facilities',
    'Proposal is out of date; ask the contributor to resubmit from the latest content',
] as const

const known = new Set<string>(QUICK_REASONS)

/**
 * Renders a stored rejection note for the current viewer: shortcut segments
 * (joined with ` · `) are translated, free text is left untouched.
 */
export function localizedReason(note: string, translate: (value: string) => string): string {
    return note
        .split(' · ')
        .map((part) => part.trim())
        .filter(Boolean)
        .map((part) => (known.has(part) ? translate(part) : part))
        .join(' · ')
}
