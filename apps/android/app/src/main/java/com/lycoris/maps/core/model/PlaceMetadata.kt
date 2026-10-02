package com.lycoris.maps.core.model

import java.time.DateTimeException
import java.time.Instant
import java.time.LocalTime
import java.time.ZoneId

enum class VenueType(val wireValue: String, private val zh: String, private val en: String) {
    METRO("metro", "地铁", "Metro"),
    HOSPITAL("hospital", "医院", "Hospital"),
    MALL("mall", "商场", "Mall"),
    RAILWAY_STATION("railway_station", "火车站", "Railway station"),
    SCHOOL("school", "学校", "School"),
    PUBLIC_TOILET("public_toilet", "公共卫生间", "Public toilet"),
    PARK("park", "公园", "Park"),
    AIRPORT("airport", "机场", "Airport"),
    OTHER("other", "其他", "Other");

    fun label(chinese: Boolean) = if (chinese) zh else en
    companion object {
        fun fromWire(value: String?) = entries.firstOrNull { it.wireValue == value }
    }
}

val Marker.venue: VenueType? get() = if ("accessible_toilet" in facilityCategories) VenueType.fromWire(venueType) else null

enum class OpeningStatus { UNKNOWN, SCHEDULED, OPEN, CLOSED, CLOSING_SOON }

private val hoursPattern = Regex("(?:[01][0-9]|2[0-3]):[0-5][0-9]")
private fun time(value: String?): LocalTime? = value?.takeIf(hoursPattern::matches)?.let(LocalTime::parse)

/** Matches Web/API daily hours: inclusive opening, exclusive closing, equal endpoints mean 24h. */
fun Marker.openingStatus(now: Instant): OpeningStatus {
    if (!openingHoursNote.isNullOrBlank()) return OpeningStatus.SCHEDULED
    val start = time(openTimeStart) ?: return OpeningStatus.UNKNOWN
    val end = time(openTimeEnd) ?: return OpeningStatus.UNKNOWN
    if (start == end) return OpeningStatus.OPEN
    // Missing server zone is not permission to infer availability from the viewer's zone.
    val zone = try { hoursTimezone?.takeIf(String::isNotBlank)?.let(ZoneId::of) } catch (_: DateTimeException) { null }
        ?: return OpeningStatus.SCHEDULED
    val current = now.atZone(zone).toLocalTime()
    val open = if (start < end) current >= start && current < end else current >= start || current < end
    if (!open) return OpeningStatus.CLOSED
    val remaining = Math.floorMod(end.toSecondOfDay() - current.toSecondOfDay(), 86400)
    return if (remaining in 1..1800) OpeningStatus.CLOSING_SOON else OpeningStatus.OPEN
}

fun Marker.hoursLabel(chinese: Boolean): String? {
    if (!openingHoursNote.isNullOrBlank()) return if (chinese) "开放时间见备注" else "See opening hours note"
    val start = time(openTimeStart) ?: return null
    val end = time(openTimeEnd) ?: return null
    return if (start == end) { if (chinese) "全天开放" else "Open 24 hours" } else "$openTimeStart–$openTimeEnd"
}
