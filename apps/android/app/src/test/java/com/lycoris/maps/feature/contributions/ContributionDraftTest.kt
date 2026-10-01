package com.lycoris.maps.feature.contributions

import com.lycoris.maps.core.model.Marker
import com.lycoris.maps.core.network.CreateMarkerRequest
import com.lycoris.maps.core.network.EditMarkerRequest
import com.lycoris.maps.core.network.LycorisJson
import kotlinx.serialization.encodeToString
import java.util.UUID
import org.junit.Assert.*
import org.junit.Test

class ContributionDraftTest {
    @Test fun englishClientAndRestoredEnglishDraftSubmitChineseContent() {
        val draft = ContributionDraft(UUID.randomUUID().toString(), "a", "https://example.test/", 31.2, 121.5,
            ContributionFields(title = "中文点位", language = "en"))
        assertEquals("zh", LycorisJson.decodeFromString<CreateMarkerRequest>(draft.frozenBody()).language)
        val marker = Marker(17, 31.2, 121.5, "accessible_toilet", "原文")
        assertEquals("zh", LycorisJson.decodeFromString<EditMarkerRequest>(draft.copy(original = marker).frozenBody()).language)
    }

    @Test fun venueSurvivesEditAndDraftRestoreAndCategoryChangesClearIt() {
        val marker = Marker(17, 31.2, 121.5, "accessible_toilet", "Place", venueType = "metro", hoursTimezone = "Asia/Shanghai")
        val draft = ContributionDraft(UUID.randomUUID().toString(), "a", "https://example.test/", marker.lat, marker.lng,
            ContributionFields.fromMarker(marker), original = marker)
        assertFalse(draft.hasTextChanges)
        val restored = LycorisJson.decodeFromString<ContributionDraft>(LycorisJson.encodeToString(draft))
        assertEquals("metro", restored.fields.venueType)
        val changed = restored.copy(fields = restored.fields.copy(title = "Updated"))
        assertEquals("metro", LycorisJson.decodeFromString<EditMarkerRequest>(changed.frozenBody()).venueType)
        val other = changed.copy(fields = changed.fields.withCategory("baby_room"))
        assertNull(other.fields.venueType)
        assertNull(LycorisJson.decodeFromString<EditMarkerRequest>(other.frozenBody()).venueType)
        assertFalse(other.fields.copy(venueType = "metro").isValid())
        assertFalse(draft.fields.copy(venueType = "invalid").isValid())
        val legacy = LycorisJson.decodeFromString<ContributionFields>("""{"title":"Legacy draft"}""")
        assertTrue(legacy.isValid())
        val newDraft = draft.copy(original = null, markerId = null, fields = legacy)
        assertEquals("other", LycorisJson.decodeFromString<CreateMarkerRequest>(newDraft.frozenBody()).venueType)
        assertEquals("school", LycorisJson.decodeFromString<CreateMarkerRequest>(newDraft.copy(fields = legacy.copy(venueType = "school")).frozenBody()).venueType)
        val oldMarker = marker.copy(venueType = null)
        val oldEdit = draft.copy(original = oldMarker, fields = ContributionFields.fromMarker(oldMarker).copy(title = "Updated legacy draft"))
        assertFalse("Legacy edits must not overwrite a classification added on the server", oldEdit.frozenBody().contains("venueType"))
    }

    @Test fun newVenueTypesSurviveCreateEditAndDraftPersistence() {
        for (venue in listOf("public_toilet", "airport")) {
            val marker = Marker(17, 31.2, 121.5, "accessible_toilet", "Place", venueType = venue)
            val draft = ContributionDraft(UUID.randomUUID().toString(), "a", "https://example.test/", marker.lat, marker.lng,
                ContributionFields.fromMarker(marker), original = marker)
            val restored = LycorisJson.decodeFromString<ContributionDraft>(LycorisJson.encodeToString(draft))
            assertTrue(restored.fields.isValid())
            assertEquals(venue, LycorisJson.decodeFromString<EditMarkerRequest>(restored.copy(fields = restored.fields.copy(title = "Updated")).frozenBody()).venueType)
            assertEquals(venue, LycorisJson.decodeFromString<CreateMarkerRequest>(restored.copy(original = null).frozenBody()).venueType)
        }
    }

    @Test fun validationCountsUnicodeScalarsAndRequiresCompleteHours() {
        val fields = ContributionFields(title = "😀".repeat(120))
        assertTrue(fields.isValid())
        assertFalse(fields.copy(title = "😀".repeat(121)).isValid())
        assertFalse(fields.copy(title = " \n ").isValid())
        assertFalse(fields.copy(openTimeStart = "09:00").isValid())
        assertFalse(fields.copy(openTimeStart = "24:00", openTimeEnd = "12:00").isValid())
        assertTrue(fields.copy(openTimeStart = "09:00", openTimeEnd = "23:59").isValid())
        assertFalse(fields.copy(category = "unexpected").isValid())
    }
    @Test fun frozenCreationContainsExactKeyAndNoPrivateVisibilityOrImageField() {
        val draft = ContributionDraft(UUID.randomUUID().toString(), "a", "https://example.test/", 31.2, 121.5, ContributionFields(title = "  Place  "))
        val raw = draft.frozenBody()
        val decoded = LycorisJson.decodeFromString<CreateMarkerRequest>(raw)
        assertEquals("Place", decoded.title)
        assertEquals(draft.creationRequestId, decoded.clientRequestId)
        assertFalse(raw.contains("isPublic"))
        assertFalse(raw.contains("markImage"))
        val frozen = draft.copy(phase = DraftPhase.CREATING, frozenRequest = raw)
        assertFalse(frozen.editable)
        assertTrue(frozen.safelyResumable)
        assertEquals(raw, frozen.copy(attempts = 3).frozenRequest)
    }
    @Test fun unchangedEditDoesNotCreateTextProposalAndUncertaintyIsNeverResumable() {
        val marker = Marker(17, 31.2, 121.5, "baby_room", "Place")
        val draft = ContributionDraft(UUID.randomUUID().toString(), "a", "https://example.test/", marker.lat, marker.lng, ContributionFields.fromMarker(marker), original = marker)
        assertFalse(draft.hasTextChanges)
        assertFalse(draft.canSubmit)
        assertTrue(draft.copy(fields = draft.fields.copy(title = "Updated")).canSubmit)
        assertFalse(draft.copy(phase = DraftPhase.UNCERTAIN_EDIT).safelyResumable)
        assertFalse(draft.copy(latitude = 0.0).isValidCheckpoint())
    }
}
