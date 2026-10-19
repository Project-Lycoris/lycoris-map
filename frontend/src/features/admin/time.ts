import type { Language } from '@/shared/i18n'

const locale = (language: Language) => (language === 'zh' ? 'zh-CN' : 'en-US')

/**
 * Localized absolute timestamp for the `title`/details of a queue row.
 * Invalid timestamps are returned untouched so a broken server value is
 * visible instead of silently becoming "now".
 */
export function fullTime(value: string, language: Language): string {
    const date = new Date(value)
    if (Number.isNaN(date.getTime())) return value
    return new Intl.DateTimeFormat(locale(language), {
        dateStyle: 'medium',
        timeStyle: 'short',
    }).format(date)
}

const units: [Intl.RelativeTimeFormatUnit, number][] = [
    ['year', 365 * 24 * 60 * 60],
    ['month', 30 * 24 * 60 * 60],
    ['week', 7 * 24 * 60 * 60],
    ['day', 24 * 60 * 60],
    ['hour', 60 * 60],
    ['minute', 60],
    ['second', 1],
]

/** "3 hours ago" / "3 小时前", using the dashboard language. */
export function relativeTime(value: string, language: Language, now = Date.now()): string {
    const date = new Date(value)
    if (Number.isNaN(date.getTime())) return value
    const seconds = Math.round((date.getTime() - now) / 1000)
    const absolute = Math.abs(seconds)
    const formatter = new Intl.RelativeTimeFormat(locale(language), { numeric: 'auto' })
    // Timestamps slightly in the future (clock skew) must read as "just now",
    // never as "in 0 seconds" / "0 秒钟后".
    if (absolute < 45) return language === 'zh' ? '刚刚' : 'just now'
    for (const [unit, size] of units) {
        if (absolute >= size || unit === 'second')
            return formatter.format(Math.round(seconds / size), unit)
    }
    return formatter.format(0, 'second')
}
