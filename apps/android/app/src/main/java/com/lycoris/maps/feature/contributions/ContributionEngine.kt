package com.lycoris.maps.feature.contributions

import com.lycoris.maps.core.data.SessionIdentity
import com.lycoris.maps.core.data.drafts.DraftLocks
import com.lycoris.maps.core.data.drafts.DraftStore
import com.lycoris.maps.core.media.PhotoFailure
import com.lycoris.maps.core.media.PhotoFiles
import com.lycoris.maps.core.media.PhotoPolicy
import com.lycoris.maps.core.network.ApiFailure
import com.lycoris.maps.core.network.CreateUploadRequest
import com.lycoris.maps.core.network.UploadReceipt
import java.io.RandomAccessFile
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

enum class ContributionRunResult { DONE, RETRY, PAUSED }

/** Durable protocol engine; Android scheduling and UI are adapters, never sources of upload truth. */
class ContributionEngine(
    private val store: DraftStore,
    private val locks: DraftLocks,
    private val transport: ContributionTransport,
    private val photos: PhotoFiles,
) {
    companion object { const val MAX_RETRIES = 6 }

    suspend fun resume(id: String): ContributionRunResult = locks.forDraft(id).withLock {
        var draft = store.get(id) ?: return@withLock ContributionRunResult.DONE
        if (!draft.isValidCheckpoint()) throw DraftStorageFailure()
        if (draft.phase == DraftPhase.EDITING) {
            store.update(draft, draft.copy(phase = DraftPhase.UNCERTAIN_EDIT, problem = DraftProblem.UNCERTAIN_EDIT, paused = true))
            return@withLock ContributionRunResult.PAUSED
        }
        if (!draft.safelyResumable || draft.paused) return@withLock if (draft.phase == DraftPhase.COMPLETE) ContributionRunResult.DONE else ContributionRunResult.PAUSED
        try {
            transport.restoreSession()
            val identity = matchingIdentity(draft) ?: run {
                store.update(draft, draft.copy(paused = true, problem = DraftProblem.SESSION_REQUIRED))
                return@withLock ContributionRunResult.PAUSED
            }
            if (draft.phase == DraftPhase.CREATING) {
                currentCoroutineContext().ensureActive()
                val marker = transport.create(identity, draft.frozenRequest!!, draft.fields.language)
                if (!marker.hasValidLocation || marker.userPublicId != draft.owner || marker.clientRequestId != draft.creationRequestId ||
                    marker.lat != draft.latitude || marker.lng != draft.longitude) throw ApiFailure.InvalidResponse()
                // Save the receipt before any image request. Never recreate a point because its photo failed.
                draft = store.update(draft, draft.copy(
                    markerId = marker.id, phase = if (draft.photo == null) DraftPhase.COMPLETE else DraftPhase.UPLOADING,
                    attempts = 0, problem = null,
                ))
            }
            while (draft.phase == DraftPhase.UPLOADING) {
                upload(draft, identity)
                draft = store.get(id) ?: throw DraftStorageFailure()
            }
            ContributionRunResult.DONE
        } catch (cancelled: CancellationException) {
            // Creation/chunks/completion are idempotent; leave their exact durable checkpoint for reconciliation.
            throw cancelled
        } catch (failure: Exception) {
            handleFailure(store.get(id) ?: return@withLock ContributionRunResult.DONE, failure)
        }
    }

    /** Only called once by submit while holding the same draft lock, immediately after persisting EDITING. */
    internal suspend fun firstEdit(draft: ContributionDraft, identity: SessionIdentity): ContributionRunResult {
        require(draft.phase == DraftPhase.EDITING && draft.original != null)
        try {
            if (matchingIdentity(draft) != identity) {
                store.update(draft, draft.copy(phase = DraftPhase.DRAFT, frozenRequest = null, problem = DraftProblem.SESSION_REQUIRED, paused = true))
                return ContributionRunResult.PAUSED
            }
            val marker = transport.edit(identity, draft.markerId!!, draft.frozenRequest!!, draft.fields.language)
            if (marker.id != draft.markerId || !marker.hasValidLocation) throw ApiFailure.InvalidResponse()
            // PATCH returns the original public marker. Success means a pending proposal, not a published edit.
            store.update(draft, draft.copy(
                phase = if (draft.photo == null) DraftPhase.COMPLETE else DraftPhase.UPLOADING,
                attempts = 0, problem = null,
            ))
            return ContributionRunResult.DONE
        } catch (cancelled: CancellationException) {
            withContext(NonCancellable) { markUncertain(draft.id) }
            throw cancelled
        } catch (failure: Exception) {
            val latest = store.get(draft.id) ?: return ContributionRunResult.DONE
            if (latest.phase != DraftPhase.EDITING) return handleFailure(latest, failure)
            val definitelyRejected = failure is ApiFailure.Http && failure.status in 400..499 && failure.status != 408
            store.update(latest, latest.copy(
                phase = if (definitelyRejected) DraftPhase.DRAFT else DraftPhase.UNCERTAIN_EDIT,
                frozenRequest = if (definitelyRejected) null else latest.frozenRequest,
                problem = if (definitelyRejected) rejectionProblem(failure as ApiFailure.Http) else DraftProblem.UNCERTAIN_EDIT,
                paused = true,
            ))
            return ContributionRunResult.PAUSED
        }
    }

    private suspend fun markUncertain(id: String) {
        val latest = store.get(id) ?: return
        if (latest.phase == DraftPhase.EDITING) store.update(latest, latest.copy(
            phase = DraftPhase.UNCERTAIN_EDIT, problem = DraftProblem.UNCERTAIN_EDIT, paused = true,
        ))
    }

    private suspend fun upload(initial: ContributionDraft, identity: SessionIdentity): ContributionRunResult {
        var draft = initial
        val photo = draft.photo ?: throw PhotoFailure.Missing()
        val markerId = draft.markerId ?: throw InvalidUploadReceipt()
        fun requireSameSession() {
            if (matchingIdentity(draft) != identity) throw ApiFailure.SessionChanged()
        }
        requireSameSession()
        // Reconcile first, including completed receipts even if the local image has since disappeared.
        var receipt = if (draft.upload == null) transport.beginUpload(identity, markerId, CreateUploadRequest(
            photo.id, photo.byteCount, photo.sha256,
        )) else transport.uploadStatus(identity, markerId, draft.upload!!.uploadId)
        draft = saveReceipt(draft, receipt)
        if (receipt.status == "COMPLETED") return finish(draft)
        val file = photos.verify(photo)
        var stalled = 0
        RandomAccessFile(file, "r").use { input ->
            while (receipt.receivedBytes < receipt.totalBytes) {
                currentCoroutineContext().ensureActive()
                requireSameSession()
                val offset = receipt.receivedBytes
                val length = minOf(PhotoPolicy.UPLOAD_CHUNK_BYTES, receipt.totalBytes - offset)
                val bytes = ByteArray(length)
                input.seek(offset.toLong())
                input.readFully(bytes)
                val next = try {
                    transport.chunk(identity, markerId, receipt.uploadId, offset, bytes)
                } catch (conflict: ApiFailure.Http) {
                    if (conflict.status != 409) throw conflict
                    // A duplicate/concurrent chunk may have been accepted. Query; do not blindly replay it.
                    transport.uploadStatus(identity, markerId, receipt.uploadId)
                }
                draft = saveReceipt(draft, next)
                stalled = if (next.receivedBytes == receipt.receivedBytes) stalled + 1 else 0
                receipt = next
                if (receipt.status == "COMPLETED") return finish(draft)
                if (stalled > 1) throw ApiFailure.Http(409)
            }
        }
        requireSameSession()
        receipt = transport.complete(identity, markerId, receipt.uploadId)
        draft = saveReceipt(draft, receipt)
        if (receipt.status != "COMPLETED") throw InvalidUploadReceipt()
        return finish(draft)
    }

    private suspend fun saveReceipt(draft: ContributionDraft, receipt: UploadReceipt): ContributionDraft {
        if (!UploadReceiptPolicy.valid(receipt, draft)) throw InvalidUploadReceipt()
        return store.update(draft, draft.copy(upload = receipt, problem = null))
    }

    private suspend fun finish(draft: ContributionDraft): ContributionRunResult {
        val next = draft.queuedPhotos.firstOrNull()
        if (next != null) {
            store.update(draft, draft.copy(photo = next, queuedPhotos = draft.queuedPhotos.drop(1), upload = null,
                uploadedPhotoCount = draft.uploadedPhotoCount + 1, paused = false, attempts = 0, problem = null))
            draft.photo?.let(photos::delete)
        } else store.update(draft, draft.copy(phase = DraftPhase.COMPLETE, paused = false, attempts = 0, problem = null))
        // Keep the encoded file until the user discards this local record. A COMPLETE checkpoint remains valid.
        return ContributionRunResult.DONE
    }

    private fun matchingIdentity(draft: ContributionDraft): SessionIdentity? = transport.identity()?.takeIf {
        it.publicId == draft.owner && it.origin == draft.origin
    }

    private suspend fun handleFailure(draft: ContributionDraft, failure: Exception): ContributionRunResult {
        val status = (failure as? ApiFailure.Http)?.status
        val session = failure is ApiFailure.SessionChanged || failure is ApiFailure.SessionRequired || status == 401
        val transient = failure is ApiFailure.Network || status == 408 || status == 429 || (status != null && status in 500..599)
        val nextAttempts = draft.attempts + 1
        val retry = draft.safelyResumable && transient && nextAttempts < MAX_RETRIES
        val problem = when {
            session -> DraftProblem.SESSION_REQUIRED
            failure is InvalidUploadReceipt || failure is ApiFailure.InvalidResponse -> DraftProblem.INVALID_RECEIPT
            failure is PhotoFailure.Missing -> DraftProblem.MISSING_PHOTO
            failure is DraftStorageFailure -> DraftProblem.STORAGE
            status == 410 -> DraftProblem.PHOTO_EXPIRED
            status == 404 -> DraftProblem.INACCESSIBLE
            status == 409 -> DraftProblem.CONFLICT
            status in setOf(400, 413, 415) && draft.phase == DraftPhase.UPLOADING -> DraftProblem.PHOTO_REJECTED
            transient && !retry -> DraftProblem.RETRY_LIMIT
            transient -> DraftProblem.NETWORK
            else -> DraftProblem.INVALID_FIELDS
        }
        // Rejected creation is editable; an ambiguous creation retains its frozen idempotency key.
        val rejectedCreate = draft.phase == DraftPhase.CREATING && status in setOf(400, 413, 415, 422)
        store.update(draft, draft.copy(
            phase = if (rejectedCreate) DraftPhase.DRAFT else draft.phase,
            frozenRequest = if (rejectedCreate) null else draft.frozenRequest,
            creationRequestId = if (rejectedCreate) UUID.randomUUID().toString() else draft.creationRequestId,
            problem = problem, paused = !retry, attempts = nextAttempts,
        ))
        return if (retry) ContributionRunResult.RETRY else ContributionRunResult.PAUSED
    }

    private fun rejectionProblem(failure: ApiFailure.Http): DraftProblem = when (failure.status) {
        401 -> DraftProblem.SESSION_REQUIRED
        404 -> DraftProblem.INACCESSIBLE
        409 -> DraftProblem.CONFLICT
        else -> DraftProblem.INVALID_FIELDS
    }
}
