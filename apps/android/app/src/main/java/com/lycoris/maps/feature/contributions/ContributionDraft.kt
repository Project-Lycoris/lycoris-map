package com.lycoris.maps.feature.contributions

import com.lycoris.maps.core.media.EncodedPhoto
import com.lycoris.maps.core.media.PhotoPolicy
import com.lycoris.maps.core.media.canonicalUuid
import com.lycoris.maps.core.model.Marker
import com.lycoris.maps.core.model.VenueType
import com.lycoris.maps.core.model.PlaceCategory
import com.lycoris.maps.core.model.validCoordinate
import com.lycoris.maps.core.network.CreateMarkerRequest
import com.lycoris.maps.core.network.EditMarkerRequest
import com.lycoris.maps.core.network.LycorisJson
import com.lycoris.maps.core.network.UploadReceipt
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString

@Serializable
data class ContributionFields(
    val title: String = "",
    val category: String = PlaceCategory.ACCESSIBLE_TOILET.wireValue,
    val description: String = "",
    val openTimeStart: String = "",
    val openTimeEnd: String = "",
    val language: String = "zh",
    val venueType: String? = null,
) {
    fun isValid(): Boolean = title.trim().isNotEmpty() && title.trim().let { it.codePointCount(0, it.length) <= 120 } &&
        (venueType == null || (category == PlaceCategory.ACCESSIBLE_TOILET.wireValue && VenueType.fromWire(venueType) != null)) &&
        category in PlaceCategory.entries.map { it.wireValue } && language in setOf("en", "zh") &&
        ((openTimeStart.isEmpty() && openTimeEnd.isEmpty()) || (validTime(openTimeStart) && validTime(openTimeEnd)))

    val submittedVenueType: String? get() = if (category == PlaceCategory.ACCESSIBLE_TOILET.wireValue) venueType ?: VenueType.OTHER.wireValue else null

    fun withCategory(value: String) = copy(category = value, venueType = if (value == PlaceCategory.ACCESSIBLE_TOILET.wireValue) venueType else null)

    companion object {
        fun fromMarker(marker: Marker): ContributionFields = ContributionFields(
            marker.title, marker.category, marker.description.orEmpty(), marker.openTimeStart.orEmpty(),
            marker.openTimeEnd.orEmpty(), "zh", marker.venueType,
        )
        private fun validTime(value: String) = value.matches(Regex("(?:[01][0-9]|2[0-3]):[0-5][0-9]"))
    }
}

@Serializable
enum class DraftPhase { DRAFT, CREATING, EDITING, UNCERTAIN_EDIT, UPLOADING, COMPLETE }
@Serializable
enum class DraftProblem { SESSION_REQUIRED, NETWORK, RETRY_LIMIT, INVALID_RECEIPT, INVALID_FIELDS, MISSING_PHOTO, PHOTO_EXPIRED, PHOTO_REJECTED, INACCESSIBLE, CONFLICT, STORAGE, UNCERTAIN_EDIT }

@Serializable
data class ContributionDraft(
    val id: String,
    val owner: String,
    val origin: String,
    val latitude: Double,
    val longitude: Double,
    val fields: ContributionFields,
    val original: Marker? = null,
    val creationRequestId: String = id,
    val phase: DraftPhase = DraftPhase.DRAFT,
    val frozenRequest: String? = null,
    val markerId: Long? = original?.id,
    val photo: EncodedPhoto? = null,
    val upload: UploadReceipt? = null,
    val problem: DraftProblem? = null,
    val paused: Boolean = false,
    val attempts: Int = 0,
    val revision: Long = 0,
    val updatedAt: Long = 0,
) {
    val editable: Boolean get() = phase == DraftPhase.DRAFT
    val hasTextChanges: Boolean get() = original == null || fields != ContributionFields.fromMarker(original)
    val canSubmit: Boolean get() = editable && fields.isValid() && (hasTextChanges || photo != null)
    val safelyResumable: Boolean get() = phase == DraftPhase.CREATING || phase == DraftPhase.UPLOADING
    val canReplacePhoto: Boolean get() = editable || (phase == DraftPhase.UPLOADING &&
        problem in setOf(DraftProblem.PHOTO_EXPIRED, DraftProblem.PHOTO_REJECTED, DraftProblem.MISSING_PHOTO))

    fun frozenBody(): String {
        require(canSubmit)
        return if (original == null) LycorisJson.encodeToString(CreateMarkerRequest(
            latitude, longitude, fields.category, fields.title.trim(), fields.description, "zh",
            fields.openTimeStart, fields.openTimeEnd, creationRequestId, venueType = fields.submittedVenueType,
        )) else LycorisJson.encodeToString(EditMarkerRequest(
            // Older saved drafts have no venue field. Omit it to preserve the server's later
            // classification; the API itself clears it when changing to another category.
            fields.category, fields.title.trim(), fields.description, "zh", fields.openTimeStart, fields.openTimeEnd, venueType = fields.venueType,
        ))
    }

    fun isValidCheckpoint(): Boolean {
        if (canonicalUuid(id) != id || canonicalUuid(creationRequestId) != creationRequestId || owner.isBlank() || origin.isBlank() || !validCoordinate(latitude, longitude) ||
            markerId?.let { it <= 0L } == true || revision < 0 || attempts < 0) return false
        if (original != null && (markerId != original.id || latitude != original.lat || longitude != original.lng)) return false
        if (phase in setOf(DraftPhase.CREATING, DraftPhase.EDITING, DraftPhase.UNCERTAIN_EDIT) && frozenRequest.isNullOrBlank()) return false
        if (phase == DraftPhase.CREATING && original != null) return false
        if (phase in setOf(DraftPhase.EDITING, DraftPhase.UNCERTAIN_EDIT) && original == null) return false
        if (phase in setOf(DraftPhase.UPLOADING, DraftPhase.COMPLETE) && markerId == null) return false
        if (phase == DraftPhase.UPLOADING && photo == null) return false
        if (photo != null && !photo.isValid()) return false
        if (upload != null && !UploadReceiptPolicy.valid(upload, this, previous = null)) return false
        return true
    }
}

object UploadReceiptPolicy {
    fun valid(receipt: UploadReceipt, draft: ContributionDraft, previous: UploadReceipt? = draft.upload): Boolean {
        val photo = draft.photo ?: return false
        val normalizedId = canonicalUuid(receipt.uploadId) ?: return false
        return receipt.markerId == draft.markerId && receipt.totalBytes == photo.byteCount &&
            receipt.chunkSize == PhotoPolicy.UPLOAD_CHUNK_BYTES && receipt.receivedBytes in 0..receipt.totalBytes &&
            (receipt.receivedBytes == receipt.totalBytes || receipt.receivedBytes % receipt.chunkSize == 0) &&
            receipt.status in setOf("UPLOADING", "COMPLETED") &&
            (receipt.status != "COMPLETED" || receipt.receivedBytes == receipt.totalBytes) &&
            (previous == null || (normalizedId == canonicalUuid(previous.uploadId) &&
                receipt.receivedBytes >= previous.receivedBytes &&
                (previous.status != "COMPLETED" || receipt.status == "COMPLETED")))
    }
}

class DraftStorageFailure : Exception("Could not save contribution draft")
class InvalidUploadReceipt : Exception("Upload receipt does not match this draft")
