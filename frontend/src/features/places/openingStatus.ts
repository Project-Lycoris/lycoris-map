import { useEffect, useState } from 'react'
import type { Marker } from '@/shared/api/markers'

type Hours = Pick<Marker, 'openTimeStart' | 'openTimeEnd' | 'hoursTimezone' | 'openingHoursNote'>
export type OpeningStatus = 'unknown' | 'scheduled' | 'open' | 'closed' | 'closing-soon'
const timePattern = /^(?:[01]\d|2[0-3]):[0-5]\d$/
const seconds = (time: string) => Number(time.slice(0, 2)) * 3600 + Number(time.slice(3)) * 60

/** Same daily/overnight and equal-time (24h) semantics as the Rust API. */
export function openingStatus(place: Hours | null | undefined, now: Date): OpeningStatus {
    if (place?.openingHoursNote?.trim()) return 'scheduled'
    const start = place?.openTimeStart,
        end = place?.openTimeEnd
    if (!start || !end || !timePattern.test(start) || !timePattern.test(end)) return 'unknown'
    if (start === end) return 'open'
    // Never use the viewer's time zone to infer a distant place's availability.
    if (!place?.hoursTimezone || !Number.isFinite(now.getTime())) return 'scheduled'
    let current: number
    try {
        const parts = new Intl.DateTimeFormat('en-GB', {
            timeZone: place.hoursTimezone,
            hourCycle: 'h23',
            hour: '2-digit',
            minute: '2-digit',
            second: '2-digit',
        }).formatToParts(now)
        const part = (name: string) => Number(parts.find((item) => item.type === name)?.value)
        current = part('hour') * 3600 + part('minute') * 60 + part('second')
        if (!Number.isFinite(current)) return 'scheduled'
    } catch {
        return 'scheduled'
    }
    const from = seconds(start),
        to = seconds(end)
    const open = from < to ? current >= from && current < to : current >= from || current < to
    if (!open) return 'closed'
    const remaining = (to - current + 86400) % 86400
    return remaining > 0 && remaining <= 30 * 60 ? 'closing-soon' : 'open'
}
export function hoursStatusLabel(status: OpeningStatus): string {
    return status === 'open' || status === 'closing-soon'
        ? 'Open now'
        : status === 'closed'
          ? 'Closed now'
          : ''
}
export function useOpeningStatus(place: Hours | null | undefined): OpeningStatus {
    const [now, setNow] = useState(() => new Date())
    useEffect(() => {
        let timer: ReturnType<typeof setTimeout>
        const tick = () => {
            clearTimeout(timer)
            setNow(new Date())
            timer = setTimeout(tick, 60000 - (Date.now() % 60000))
        }
        const visible = () => {
            if (document.visibilityState === 'visible') tick()
        }
        tick()
        document.addEventListener('visibilitychange', visible)
        window.addEventListener('pageshow', tick)
        return () => {
            clearTimeout(timer)
            document.removeEventListener('visibilitychange', visible)
            window.removeEventListener('pageshow', tick)
        }
    }, [])
    return openingStatus(place, now)
}
