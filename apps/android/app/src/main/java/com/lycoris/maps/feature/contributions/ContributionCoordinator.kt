package com.lycoris.maps.feature.contributions

import android.net.Uri
import com.lycoris.maps.core.data.AccountRepository
import com.lycoris.maps.core.data.SessionIdentity
import com.lycoris.maps.core.data.drafts.DraftLocks
import com.lycoris.maps.core.data.drafts.DraftStore
import com.lycoris.maps.core.media.EncodedPhoto
import com.lycoris.maps.core.media.PhotoImporter
import com.lycoris.maps.core.model.Marker
import com.lycoris.maps.core.model.validCoordinate
import com.lycoris.maps.core.network.ApiFailure
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.cancel
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/** Application-scoped coordinator. Closing a form does not destroy its draft or its idempotent upload. */
class ContributionCoordinator(
    private val accounts: AccountRepository,
    private val store: DraftStore,
    private val locks: DraftLocks,
    private val importer: PhotoImporter,
    private val engine: ContributionEngine,
    private val scheduler: ContributionScheduler,
    scope: CoroutineScope,
) {
    private val mutable = MutableStateFlow<List<ContributionDraft>>(emptyList())
    val drafts: StateFlow<List<ContributionDraft>> = mutable.asStateFlow()
    private val failure = MutableStateFlow<DraftProblem?>(null)
    val storageProblem: StateFlow<DraftProblem?> = failure.asStateFlow()

    private sealed interface FieldWrite {
        val identity: SessionIdentity
        val id: String
        data class Value(override val identity: SessionIdentity, override val id: String, val fields: ContributionFields) : FieldWrite
        data class Barrier(override val identity: SessionIdentity, override val id: String, val done: CompletableDeferred<Unit>) : FieldWrite
    }
    private val fieldWrites = Channel<FieldWrite>(Channel.UNLIMITED)

    /** Capture the account epoch now; a queued field edit must never cross a login transition. */
    fun enqueueFields(id: String, fields: ContributionFields) {
        val identity = accounts.identity()
        if (identity == null) { failure.value = DraftProblem.SESSION_REQUIRED; return }
        if (!fieldWrites.trySend(FieldWrite.Value(identity, id, fields)).isSuccess) failure.value = DraftProblem.STORAGE
    }

    /** A FIFO barrier used by submit; runs before acquiring the draft lock. */
    suspend fun flushFields(id: String) {
        val identity = accounts.identity() ?: throw ApiFailure.SessionRequired()
        val done = CompletableDeferred<Unit>()
        fieldWrites.send(FieldWrite.Barrier(identity, id, done))
        done.await()
        requireIdentity(identity)
    }

    init {
        scope.launch(Dispatchers.IO) {
            val errors = mutableMapOf<Pair<SessionIdentity, String>, Exception>()
            for (command in fieldWrites) {
                val key = command.identity to command.id
                when (command) {
                    is FieldWrite.Value -> {
                        try {
                            requireIdentity(command.identity)
                            owned(expectedIdentity = command.identity) { pinned -> writeFields(command.id, command.fields, pinned) }
                            errors.remove(key)
                            failure.value = null
                        } catch (problem: Exception) {
                            currentCoroutineContext().ensureActive()
                            errors[key] = problem
                            failure.value = if (problem is ApiFailure.SessionChanged || problem is ApiFailure.SessionRequired || problem is CancellationException)
                                DraftProblem.SESSION_REQUIRED else DraftProblem.STORAGE
                        }
                    }
                    is FieldWrite.Barrier -> {
                        val error = errors[key]
                        if (error == null && accounts.identity() == command.identity) command.done.complete(Unit)
                        else command.done.completeExceptionally(error ?: ApiFailure.SessionChanged())
                    }
                }
            }
        }
        scope.launch(Dispatchers.IO) {
            var previous: SessionIdentity? = null
            accounts.state.map { accounts.identity() }.distinctUntilChanged().collectLatest { identity ->
                mutable.value = emptyList() // Clear private content before waiting for database/work cancellation.
                failure.value = null
                val old = previous
                previous = identity
                try {
                    if (old != null) {
                        scheduler.cancelOwner(old.origin, old.publicId)
                        pauseOwner(old)
                    }
                    if (identity != null) {
                        recoverOwner(identity)
                        store.observe(identity.origin, identity.publicId).collect { values ->
                            if (accounts.identity() == identity) mutable.value = values
                        }
                    }
                } catch (cancelled: CancellationException) { throw cancelled }
                catch (_: Exception) { failure.value = DraftProblem.STORAGE }
            }
        }
    }

    suspend fun createDraft(latitude: Double, longitude: Double, language: String, original: Marker? = null): String = owned { identity ->
        require(validCoordinate(latitude, longitude) && language in setOf("en", "zh"))
        require(original == null || (original.hasValidLocation && original.lat == latitude && original.lng == longitude))
        val draft = ContributionDraft(
            id = UUID.randomUUID().toString(), owner = identity.publicId, origin = identity.origin,
            latitude = latitude, longitude = longitude,
            fields = original?.let(ContributionFields::fromMarker) ?: ContributionFields(language = "zh"),
            original = original, updatedAt = System.currentTimeMillis(),
        )
        requireIdentity(identity)
        store.insert(draft)
        draft.id
    }

    suspend fun updateFields(id: String, fields: ContributionFields) = owned { identity -> writeFields(id, fields, identity) }

    private suspend fun writeFields(id: String, fields: ContributionFields, identity: SessionIdentity) {
        locks.forDraft(id).withLock {
            val draft = requireOwned(id, identity)
            require(draft.editable)
            store.update(draft, draft.copy(fields = fields, paused = false, problem = null))
        }
    }

    suspend fun importPhoto(id: String, uri: Uri, append: Boolean = false) = owned { identity ->
        val before = locks.forDraft(id).withLock { requireOwned(id, identity).also { require(it.canReplacePhoto) } }
        var imported: EncodedPhoto? = null
        var committed = false
        try {
            val photo = importer.importPhoto(uri)
            imported = photo
            locks.forDraft(id).withLock {
                val draft = requireOwned(id, identity)
                // Text edits may continue while decoding, but another image or a submitted snapshot must not be overwritten.
                require(draft.canReplacePhoto && draft.photo == before.photo && draft.phase == before.phase)
                withContext(NonCancellable) {
                    requireIdentity(identity)
                    val appending = append && draft.editable && draft.photo != null
                    val next = if (appending) draft.copy(queuedPhotos = draft.queuedPhotos + photo)
                        else draft.copy(photo = photo, upload = null, problem = null, paused = false, attempts = 0)
                    val saved = store.update(draft, next)
                    committed = true
                    if (!appending) draft.photo?.let(importer.files::delete)
                    if (saved.safelyResumable) scheduler.enqueue(saved)
                }
            }
        } finally {
            if (!committed) imported?.let(importer.files::delete)
        }
    }

    suspend fun removePhoto(id: String, photoId: String? = null) = owned { identity ->
        locks.forDraft(id).withLock {
            val draft = requireOwned(id, identity)
            require(draft.editable)
            withContext(NonCancellable) {
                requireIdentity(identity)
                val removed = draft.remainingPhotos.firstOrNull { photoId == null || it.id == photoId }
                if (removed != null) {
                    val remaining = draft.remainingPhotos.filter { it.id != removed.id }
                    store.update(draft, draft.copy(photo = remaining.firstOrNull(), queuedPhotos = remaining.drop(1), upload = null))
                    importer.files.delete(removed)
                }
            }
        }
    }

    suspend fun submit(id: String) {
        val requestedIdentity = accounts.identity() ?: throw ApiFailure.SessionRequired()
        flushFields(id)
        owned(expectedIdentity = requestedIdentity) { identity ->
            locks.forDraft(id).withLock {
                val draft = requireOwned(id, identity)
                require(draft.canSubmit)
                draft.remainingPhotos.forEach(importer.files::verify)
                val phase = when {
                    draft.original == null -> DraftPhase.CREATING
                    draft.hasTextChanges -> DraftPhase.EDITING
                    else -> DraftPhase.UPLOADING // A photo-only proposal must never emit an unchanged PATCH.
                }
                val frozen = draft.frozenBody()
                try {
                    val saved = store.update(draft, draft.copy(phase = phase, frozenRequest = frozen, paused = false, problem = null, attempts = 0))
                    if (phase == DraftPhase.EDITING) engine.firstEdit(saved, identity)
                    val latest = store.get(id) ?: throw DraftStorageFailure()
                    if (latest.safelyResumable && !latest.paused) scheduler.enqueue(latest)
                } catch (cancelled: CancellationException) {
                    // A committed database write can outlive its suspended caller. Never strand an EDITING state
                    // or a safely resumable creation merely because the form/Activity disappeared.
                    withContext(NonCancellable) {
                        val latest = store.get(id)
                        if (latest?.phase == DraftPhase.EDITING) {
                            store.update(latest, latest.copy(phase = DraftPhase.UNCERTAIN_EDIT, paused = true, problem = DraftProblem.UNCERTAIN_EDIT))
                        } else if (latest?.safelyResumable == true && !latest.paused && accounts.identity() == identity) {
                            scheduler.enqueue(latest)
                        }
                    }
                    throw cancelled
                }
            }
        }

    }

    suspend fun retry(id: String) = owned { identity ->
        locks.forDraft(id).withLock {
            val draft = requireOwned(id, identity)
            require(draft.safelyResumable && draft.problem !in setOf(DraftProblem.PHOTO_EXPIRED, DraftProblem.PHOTO_REJECTED, DraftProblem.INACCESSIBLE))
            val saved = store.update(draft, draft.copy(paused = false, problem = null, attempts = 0))
            scheduler.enqueue(saved, replace = true)
        }
    }

    suspend fun discard(id: String) = owned { identity ->
        scheduler.cancel(id)
        locks.forDraft(id).withLock {
            val draft = requireOwned(id, identity)
            withContext(NonCancellable) {
                requireIdentity(identity)
                store.delete(draft)
                draft.remainingPhotos.forEach(importer.files::delete)
            }
        }
    }

    private suspend fun pauseOwner(identity: SessionIdentity) {
        for (snapshot in store.list(identity.origin, identity.publicId)) locks.forDraft(snapshot.id).withLock {
            val draft = store.get(snapshot.id) ?: return@withLock
            if (draft.phase == DraftPhase.EDITING) {
                store.update(draft, draft.copy(phase = DraftPhase.UNCERTAIN_EDIT, paused = true, problem = DraftProblem.UNCERTAIN_EDIT))
            } else if (draft.safelyResumable && (!draft.paused || draft.problem == DraftProblem.NETWORK)) {
                store.update(draft, draft.copy(paused = true, problem = DraftProblem.SESSION_REQUIRED))
            }
        }
    }

    private suspend fun recoverOwner(identity: SessionIdentity) {
        for (snapshot in store.list(identity.origin, identity.publicId)) locks.forDraft(snapshot.id).withLock {
            requireIdentity(identity)
            var draft = store.get(snapshot.id) ?: return@withLock
            if (draft.phase == DraftPhase.EDITING) {
                store.update(draft, draft.copy(phase = DraftPhase.UNCERTAIN_EDIT, paused = true, problem = DraftProblem.UNCERTAIN_EDIT))
            } else if (draft.safelyResumable && (!draft.paused || draft.problem == DraftProblem.SESSION_REQUIRED)) {
                draft = store.update(draft, draft.copy(paused = false, problem = null))
                scheduler.enqueue(draft)
            }
        }
    }

    private fun requireIdentity(identity: SessionIdentity) {
        if (accounts.identity() != identity) throw ApiFailure.SessionChanged()
    }
    private suspend fun requireOwned(id: String, identity: SessionIdentity): ContributionDraft {
        requireIdentity(identity)
        return store.get(id)?.takeIf { it.origin == identity.origin && it.owner == identity.publicId }
            ?: throw ApiFailure.InvalidInput("draft")
    }
    private suspend fun <T> owned(expectedIdentity: SessionIdentity? = null, block: suspend (SessionIdentity) -> T): T {
        val identity = accounts.identity() ?: throw ApiFailure.SessionRequired()
        if (expectedIdentity != null && identity != expectedIdentity) throw ApiFailure.SessionChanged()
        return withContext(Dispatchers.IO) {
            coroutineScope {
                val watcher = launch(start = CoroutineStart.UNDISPATCHED) {
                    accounts.state.map { accounts.identity() }.first { it != identity }
                    this@coroutineScope.cancel(CancellationException("Session changed"))
                }
                try { requireIdentity(identity); block(identity) }
                finally { watcher.cancel() }
            }
        }
    }
}
