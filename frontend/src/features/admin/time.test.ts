import { relativeTime } from './time'

const now = Date.parse('2026-10-20T12:00:00Z')

it('shows recent future timestamps (clock skew) as just now', () => {
    expect(relativeTime('2026-10-20T12:00:10Z', 'en', now)).toBe('just now')
    expect(relativeTime('2026-10-20T12:00:10Z', 'zh', now)).toBe('刚刚')
    expect(relativeTime('2026-10-20T11:59:50Z', 'en', now)).toBe('just now')
    expect(relativeTime('2026-10-20T11:59:50Z', 'zh', now)).toBe('刚刚')
})

it('uses natural relative wording through Intl numeric auto', () => {
    expect(relativeTime('2026-10-19T12:00:00Z', 'en', now)).toMatch(/yesterday/i)
    expect(relativeTime('2026-10-19T12:00:00Z', 'zh', now)).toBe('昨天')
    expect(relativeTime('2026-10-20T09:00:00Z', 'en', now)).toMatch(/3 hours ago/i)
})
