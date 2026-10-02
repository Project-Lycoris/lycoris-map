package com.lycoris.maps.feature.contributions

import com.lycoris.maps.core.data.SessionIdentity
import com.lycoris.maps.core.data.drafts.DraftLocks
import com.lycoris.maps.core.data.drafts.DraftStore
import com.lycoris.maps.core.media.EncodedPhoto
import com.lycoris.maps.core.media.PhotoFiles
import com.lycoris.maps.core.media.PhotoPolicy
import com.lycoris.maps.core.model.Marker
import com.lycoris.maps.core.network.ApiFailure
import com.lycoris.maps.core.network.CreateMarkerRequest
import com.lycoris.maps.core.network.CreateUploadRequest
import com.lycoris.maps.core.network.LycorisJson
import com.lycoris.maps.core.network.UploadReceipt
import java.io.File
import java.nio.file.Files
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Test

class ContributionEngineTest {
    private val owner = SessionIdentity("https://api.example.test/", "owner-a", 1)
    private fun draft() = ContributionDraft(UUID.randomUUID().toString(), owner.publicId, owner.origin, 31.2, 121.5, ContributionFields(title = "Synthetic", language = "en"))
    private fun Fixture.freeze(d: ContributionDraft = draft()): ContributionDraft = d.copy(phase = DraftPhase.CREATING, frozenRequest = d.frozenBody()).also { store.value.value = listOf(it) }
    private fun Fixture.uploading(size: Int = PhotoPolicy.UPLOAD_CHUNK_BYTES + 13): ContributionDraft {
        val d = draft()
        val id = UUID.randomUUID().toString()
        val file = File(files.directory, "$id.jpg").apply { writeBytes(ByteArray(size) { (it % 251).toByte() }) }
        val photo = EncodedPhoto(id, file.name, size, PhotoFiles.hash(file), 320, 200)
        return d.copy(phase = DraftPhase.UPLOADING, markerId = 17, photo = photo).also {
            store.value.value = listOf(it)
            remote.receipt = UploadReceipt(UUID.randomUUID().toString(), 17, size, 0, PhotoPolicy.UPLOAD_CHUNK_BYTES, "UPLOADING")
        }
    }
    private inner class Fixture : AutoCloseable {
        val store = MemoryStore()
        val files = PhotoFiles(Files.createTempDirectory("lycoris-photo-test").toFile())
        val remote = Remote()
        val engine = ContributionEngine(store, DraftLocks(), remote, files)
        override fun close() { files.directory.deleteRecursively() }
    }
    private fun scenario(block: suspend Fixture.() -> Unit) = runBlocking { Fixture().use { it.block() } }

    @Test fun uploadsAnOrderedQueueAndPreservesTheCompletedPhotoOnSecondFailure() = scenario {
        val initial = uploading(21)
        val secondId = UUID.randomUUID().toString()
        val secondFile = File(files.directory, "$secondId.jpg").apply { writeBytes(ByteArray(19) { 2 }) }
        val second = EncodedPhoto(secondId, secondFile.name, 19, PhotoFiles.hash(secondFile), 320, 200)
        store.update(initial, initial.copy(queuedPhotos = listOf(second)))
        remote.beforeBegin = {
            if (remote.begins == 2) throw ApiFailure.Network(false)
            if (remote.begins > 2) remote.receipt = UploadReceipt(UUID.randomUUID().toString(), 17, 19, 0, PhotoPolicy.UPLOAD_CHUNK_BYTES, "UPLOADING")
        }
        assertEquals(ContributionRunResult.RETRY, engine.resume(initial.id))
        val paused = store.get(initial.id)!!
        assertEquals(secondId, paused.photo!!.id)
        assertEquals(1, paused.uploadedPhotoCount)
        assertTrue(paused.queuedPhotos.isEmpty())
        assertEquals(ContributionRunResult.DONE, engine.resume(initial.id))
        assertEquals(2, remote.completions)
        assertEquals(0, remote.creates.size)
        assertEquals(DraftPhase.COMPLETE, store.get(initial.id)!!.phase)
    }

    @Test fun creationResponseLostReplaysExactBytesAndKeepsSameKey() = scenario {
        val d = freeze()
        remote.createFailure = ApiFailure.Network(true)
        assertEquals(ContributionRunResult.RETRY, engine.resume(d.id))
        val frozen = store.get(d.id)!!
        assertEquals(d.frozenRequest, frozen.frozenRequest)
        assertEquals(d.creationRequestId, frozen.creationRequestId)
        remote.createFailure = null
        assertEquals(ContributionRunResult.DONE, engine.resume(d.id))
        assertEquals(2, remote.creates.size)
        assertEquals(remote.creates[0], remote.creates[1])
        assertEquals(17L, store.get(d.id)!!.markerId)
        assertEquals(DraftPhase.COMPLETE, store.get(d.id)!!.phase)
    }

    @Test fun validCreationReceiptIsSavedBeforePhotoStarts() = scenario {
        val photoDraft = uploading(21)
        val d = freeze(draft().copy(photo = photoDraft.photo))
        remote.beforeBegin = { assertEquals(17L, store.get(d.id)!!.markerId); assertEquals(DraftPhase.UPLOADING, store.get(d.id)!!.phase) }
        assertEquals(ContributionRunResult.DONE, engine.resume(d.id))
        assertEquals(1, remote.creates.size)
    }

    @Test fun mismatchedCreationReceiptCannotStartPhotoUpload() = scenario {
        val d = freeze()
        remote.badMarkerOwner = true
        assertEquals(ContributionRunResult.PAUSED, engine.resume(d.id))
        assertEquals(DraftProblem.INVALID_RECEIPT, store.get(d.id)!!.problem)
        assertNull(store.get(d.id)!!.markerId)
        assertEquals(0, remote.begins)
    }

    @Test fun rejectedCreationGetsFreshKeyBeforeAnyPossibleEdit() = scenario {
        val d = freeze()
        remote.createFailure = ApiFailure.Http(400)
        engine.resume(d.id)
        val failed = store.get(d.id)!!
        assertEquals(DraftPhase.DRAFT, failed.phase)
        assertNotEquals(d.creationRequestId, failed.creationRequestId)
        assertNull(failed.frozenRequest)
    }

    @Test fun unknownCreateConflictStaysFrozenAndCannotBecomeEditable() = scenario {
        val d = freeze()
        remote.createFailure = ApiFailure.Http(409)
        engine.resume(d.id)
        assertEquals(DraftPhase.CREATING, store.get(d.id)!!.phase)
        assertEquals(d.frozenRequest, store.get(d.id)!!.frozenRequest)
    }

    @Test fun timedOutPatchIsUncertainAndNeverAutomaticallyReplayed() = scenario {
        val original = remote.marker(draft())
        val base = draft().copy(original = original, markerId = original.id, fields = ContributionFields.fromMarker(original).copy(title = "Changed"))
        val d = base.copy(phase = DraftPhase.EDITING, frozenRequest = base.frozenBody())
        store.insert(d)
        remote.editFailure = ApiFailure.Network(true)
        assertEquals(ContributionRunResult.PAUSED, engine.firstEdit(d, owner))
        assertEquals(DraftPhase.UNCERTAIN_EDIT, store.get(d.id)!!.phase)
        repeat(3) { engine.resume(d.id) }
        assertEquals(1, remote.edits)
    }

    @Test fun recoveredEditingCheckpointNeverIssuesPatch() = scenario {
        val original = remote.marker(draft())
        val base = draft().copy(original = original, markerId = original.id, fields = ContributionFields.fromMarker(original).copy(title = "Changed"))
        val d = base.copy(phase = DraftPhase.EDITING, frozenRequest = base.frozenBody())
        store.insert(d)
        engine.resume(d.id)
        assertEquals(0, remote.edits)
        assertEquals(DraftPhase.UNCERTAIN_EDIT, store.get(d.id)!!.phase)
    }

    @Test fun cancelledPatchPersistsUncertainty() = scenario {
        val original = remote.marker(draft())
        val base = draft().copy(original = original, markerId = original.id, fields = ContributionFields.fromMarker(original).copy(title = "Changed"))
        val d = base.copy(phase = DraftPhase.EDITING, frozenRequest = base.frozenBody())
        store.insert(d)
        remote.editFailure = CancellationException("Simulated process cancellation")
        try { engine.firstEdit(d, owner); fail("Expected cancellation") } catch (_: CancellationException) { }
        assertEquals(DraftPhase.UNCERTAIN_EDIT, store.get(d.id)!!.phase)
    }

    @Test fun resumedPhotoReconcilesAuthoritativeOffsetAndSendsOnlyTail() = scenario {
        val d = uploading()
        val receipt = remote.receipt!!
        store.update(d, d.copy(upload = receipt))
        remote.receipt = receipt.copy(receivedBytes = PhotoPolicy.UPLOAD_CHUNK_BYTES)
        assertEquals(ContributionRunResult.DONE, engine.resume(d.id))
        assertEquals(1, remote.statusReads)
        assertEquals(listOf(PhotoPolicy.UPLOAD_CHUNK_BYTES), remote.offsets)
        assertEquals(13, remote.payloads.single().size)
        assertEquals((PhotoPolicy.UPLOAD_CHUNK_BYTES % 251).toByte(), remote.payloads.single()[0])
        assertEquals(1, remote.completions)
        assertEquals(0, remote.edits)
    }

    @Test fun completionResponseLostUsesCompletedReceiptEvenWithoutLocalImage() = scenario {
        val d = uploading(17)
        remote.completeFailure = ApiFailure.Network(true)
        assertEquals(ContributionRunResult.RETRY, engine.resume(d.id))
        files.delete(d.photo!!)
        remote.completeFailure = null
        assertEquals(ContributionRunResult.DONE, engine.resume(d.id))
        assertEquals(1, remote.completions)
        assertEquals(DraftPhase.COMPLETE, store.get(d.id)!!.phase)
    }

    @Test fun duplicateChunkConflictReconcilesAndDoesNotSendSameChunkAgain() = scenario {
        val d = uploading()
        remote.chunkConflictOnce = true
        assertEquals(ContributionRunResult.DONE, engine.resume(d.id))
        assertEquals(listOf(0, PhotoPolicy.UPLOAD_CHUNK_BYTES), remote.offsets)
        assertEquals(1, remote.statusReads)
    }

    @Test fun noProgressConflictsStopWithinBoundedRequests() = scenario {
        val d = uploading()
        remote.stall = true
        assertEquals(ContributionRunResult.PAUSED, engine.resume(d.id))
        assertEquals(2, remote.offsets.size)
        assertEquals(DraftProblem.CONFLICT, store.get(d.id)!!.problem)
    }

    @Test fun sessionSwitchBetweenChunksDoesNotBorrowNewOwnerCookie() = scenario {
        val d = uploading()
        remote.afterChunk = { remote.current = owner.copy(publicId = "owner-b", epoch = 2) }
        assertEquals(ContributionRunResult.PAUSED, engine.resume(d.id))
        assertEquals(1, remote.offsets.size)
        assertEquals(DraftProblem.SESSION_REQUIRED, store.get(d.id)!!.problem)
        remote.current = owner.copy(epoch = 3)
        val paused = store.get(d.id)!!
        store.update(paused, paused.copy(paused = false, problem = null))
        remote.afterChunk = null
        assertEquals(ContributionRunResult.DONE, engine.resume(d.id))
        assertEquals(1, remote.begins)
    }

    @Test fun expiredSessionKeepsMarkerAndRequiresNewPhoto() = scenario {
        val d = uploading()
        remote.beginFailure = ApiFailure.Http(410)
        engine.resume(d.id)
        val paused = store.get(d.id)!!
        assertEquals(17L, paused.markerId)
        assertEquals(DraftProblem.PHOTO_EXPIRED, paused.problem)
        assertTrue(paused.canReplacePhoto)
        assertFalse(paused.editable)
        assertEquals(0, remote.creates.size)
    }

    @Test fun transientRetriesAreBoundedAndUnauthorizedNeverAutoRetries() = scenario {
        val d = freeze()
        remote.createFailure = ApiFailure.Http(503)
        repeat(ContributionEngine.MAX_RETRIES - 1) { assertEquals(ContributionRunResult.RETRY, engine.resume(d.id)) }
        assertEquals(ContributionRunResult.PAUSED, engine.resume(d.id))
        assertEquals(DraftProblem.RETRY_LIMIT, store.get(d.id)!!.problem)
        assertEquals(ContributionEngine.MAX_RETRIES, remote.creates.size)
        engine.resume(d.id)
        assertEquals(ContributionEngine.MAX_RETRIES, remote.creates.size)
        val next = freeze()
        remote.createFailure = ApiFailure.Http(401)
        assertEquals(ContributionRunResult.PAUSED, engine.resume(next.id))
        assertEquals(DraftProblem.SESSION_REQUIRED, store.get(next.id)!!.problem)
    }

    @Test fun receiptValidationRejectsCrossMarkerWrongSizeOffsetAndRollback() = scenario {
        val d = uploading()
        val good = remote.receipt!!
        assertTrue(UploadReceiptPolicy.valid(good, d))
        listOf(
            good.copy(markerId = 18), good.copy(totalBytes = 99), good.copy(uploadId = "not-uuid"),
            good.copy(chunkSize = 123), good.copy(receivedBytes = 1), good.copy(receivedBytes = -1),
            good.copy(receivedBytes = good.totalBytes + 1), good.copy(status = "COMPLETED"), good.copy(status = "UNKNOWN"),
        ).forEach { assertFalse(it.toString(), UploadReceiptPolicy.valid(it, d)) }
        assertFalse(UploadReceiptPolicy.valid(good, d, good.copy(receivedBytes = PhotoPolicy.UPLOAD_CHUNK_BYTES)))
        assertFalse(UploadReceiptPolicy.valid(good.copy(uploadId = UUID.randomUUID().toString()), d, good))
    }

    private class MemoryStore : DraftStore {
        val value = MutableStateFlow<List<ContributionDraft>>(emptyList())
        override fun observe(origin: String, owner: String): Flow<List<ContributionDraft>> = value.map { it.filter { d -> d.owner == owner && d.origin == origin } }
        override suspend fun get(id: String) = value.value.firstOrNull { it.id == id }
        override suspend fun list(origin: String, owner: String) = value.value.filter { it.origin == origin && it.owner == owner }
        override suspend fun insert(draft: ContributionDraft) { require(draft.isValidCheckpoint()); value.value += draft }
        override suspend fun update(expected: ContributionDraft, next: ContributionDraft): ContributionDraft {
            check(get(expected.id)?.revision == expected.revision)
            val saved = next.copy(revision = expected.revision + 1)
            require(saved.isValidCheckpoint())
            value.value = value.value.map { if (it.id == saved.id) saved else it }
            return saved
        }
        override suspend fun delete(expected: ContributionDraft) { value.value = value.value.filterNot { it.id == expected.id } }
    }

    private inner class Remote : ContributionTransport {
        var current: SessionIdentity? = owner
        var createFailure: Exception? = null
        var editFailure: Exception? = null
        var beginFailure: Exception? = null
        var completeFailure: Exception? = null
        var beforeBegin: (suspend () -> Unit)? = null
        var afterChunk: (() -> Unit)? = null
        var badMarkerOwner = false
        var chunkConflictOnce = false
        var stall = false
        var receipt: UploadReceipt? = null
        val creates = mutableListOf<String>()
        val offsets = mutableListOf<Int>()
        val payloads = mutableListOf<ByteArray>()
        var edits = 0
        var begins = 0
        var statusReads = 0
        var completions = 0
        override fun identity() = current
        override suspend fun restoreSession() { }
        fun marker(draft: ContributionDraft) = Marker(17, draft.latitude, draft.longitude, draft.fields.category, draft.fields.title, userPublicId = draft.owner, clientRequestId = draft.creationRequestId)
        override suspend fun create(identity: SessionIdentity, frozenJson: String, language: String): Marker {
            check(identity == current)
            creates += frozenJson
            createFailure?.let { throw it }
            val data = LycorisJson.decodeFromString<CreateMarkerRequest>(frozenJson)
            return Marker(17, data.lat, data.lng, data.category, data.title, userPublicId = if (badMarkerOwner) "other" else identity.publicId, clientRequestId = data.clientRequestId)
        }
        override suspend fun edit(identity: SessionIdentity, markerId: Long, frozenJson: String, language: String): Marker { edits++; editFailure?.let { throw it }; return marker(draft()) }
        override suspend fun beginUpload(identity: SessionIdentity, markerId: Long, request: CreateUploadRequest): UploadReceipt {
            check(identity == current); begins++; beforeBegin?.invoke(); beginFailure?.let { throw it }; return receipt!!
        }
        override suspend fun uploadStatus(identity: SessionIdentity, markerId: Long, uploadId: String): UploadReceipt { check(identity == current); statusReads++; return receipt!! }
        override suspend fun chunk(identity: SessionIdentity, markerId: Long, uploadId: String, offset: Int, bytes: ByteArray): UploadReceipt {
            check(identity == current); offsets += offset; payloads += bytes
            if (!stall) receipt = receipt!!.copy(receivedBytes = offset + bytes.size)
            afterChunk?.invoke()
            if (chunkConflictOnce) { chunkConflictOnce = false; throw ApiFailure.Http(409) }
            return receipt!!
        }
        override suspend fun complete(identity: SessionIdentity, markerId: Long, uploadId: String): UploadReceipt {
            check(identity == current); completions++; receipt = receipt!!.copy(status = "COMPLETED")
            completeFailure?.let { throw it }; return receipt!!
        }
    }
}
