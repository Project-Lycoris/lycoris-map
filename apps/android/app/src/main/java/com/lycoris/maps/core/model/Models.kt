package com.lycoris.maps.core.model

import kotlinx.serialization.Serializable

@Serializable
enum class Language(val tag: String) { ZH("zh"), EN("en") }

enum class PlaceCategory(val wireValue: String) {
    ACCESSIBLE_TOILET("accessible_toilet"),
    BABY_ROOM("baby_room"),
    FRIENDLY_CLINIC("friendly_clinic"),
    OTHER("self_definition");

    companion object {
        fun fromWire(value: String): PlaceCategory = entries.firstOrNull { it.wireValue == value } ?: OTHER
    }
}

@Serializable
data class Marker(
    val id: Long,
    val lat: Double,
    val lng: Double,
    val category: String,
    val title: String,
    val version: Long = 0,
    val description: String? = null,
    val sourceLanguage: String = "zh",
    val contentLanguage: String = "zh",
    val isPublic: Boolean = true,
    val username: String = "",
    val userPublicId: String? = null,
    val clientRequestId: String? = null,
    val isActive: Boolean = true,
    val openTimeStart: String? = null,
    val openTimeEnd: String? = null,
    val reviewStatus: String = "APPROVED",
    val lastEditedBy: String? = null,
    val lastEditedByPublicId: String? = null,
    val lastEditedByOwner: Boolean = false,
    val markImage: String? = null,
    val deactivated: Boolean = false,
    val createdAt: String = "",
    val updatedAt: String = "",
    val venueType: String? = null,
    val hoursTimezone: String? = null,
    val categories: List<String>? = null,
    val openingHoursNote: String? = null,
    val photos: List<MarkerPhoto> = emptyList(),
) {
    val facilityCategories: List<String> get() = categories?.takeIf { it.isNotEmpty() } ?: listOf(category)
    val placeCategory: PlaceCategory get() = PlaceCategory.fromWire(category)
    val hasValidLocation: Boolean get() = id > 0 && validCoordinate(lat, lng)
    val publiclyVisible: Boolean get() = isPublic && reviewStatus == "APPROVED" && !deactivated
}

@Serializable
data class MarkerPhoto(val id: Long, val url: String, val sortOrder: Long)

@Serializable
data class User(
    val publicId: String,
    val username: String? = null,
    val nickname: String? = null,
    val email: String? = null,
    val avatarUrl: String? = null,
    val pronouns: String? = null,
    val signature: String? = null,
) {
    val displayName: String get() = nickname?.takeIf { it.isNotBlank() }
        ?: username?.takeIf { it.isNotBlank() } ?: "Lycoris"
    override fun toString(): String = "User(publicId=$publicId)"
}

fun validCoordinate(lat: Double, lng: Double): Boolean =
    lat.isFinite() && lng.isFinite() && lat in -90.0..90.0 && lng in -180.0..180.0

/** West > east denotes a date-line crossing, split before calling the server. */
data class GeoBounds(val south: Double, val north: Double, val west: Double, val east: Double) {
    init {
        require(validCoordinate(south, west) && validCoordinate(north, east) && south <= north)
    }

    fun segments(): List<GeoBounds> = if (west <= east) listOf(this) else listOf(
        GeoBounds(south, north, west, 180.0), GeoBounds(south, north, -180.0, east),
    )
}

fun Iterable<Marker>.validMarkers(): List<Marker> = filter { it.hasValidLocation && !it.deactivated }.distinctBy { it.id }
