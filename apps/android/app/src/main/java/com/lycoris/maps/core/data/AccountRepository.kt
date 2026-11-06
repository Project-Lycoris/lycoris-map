package com.lycoris.maps.core.data

import com.lycoris.maps.core.model.Language
import com.lycoris.maps.core.model.Marker
import com.lycoris.maps.core.model.User
import com.lycoris.maps.core.model.validMarkers
import com.lycoris.maps.core.network.ApiClients
import com.lycoris.maps.core.network.ApiFailure
import com.lycoris.maps.core.network.ChangePasswordRequest
import com.lycoris.maps.core.network.LoginRequest
import com.lycoris.maps.core.network.LycorisApi
import com.lycoris.maps.core.network.RegisterRequest
import com.lycoris.maps.core.network.EmailCodeRequest
import com.lycoris.maps.core.network.ResetPasswordRequest
import com.lycoris.maps.core.network.SessionCookieJar
import com.lycoris.maps.core.network.UpdateProfileRequest
import com.lycoris.maps.core.network.apiCall
import com.lycoris.maps.core.network.requireBody
import com.lycoris.maps.core.network.requireEnvelope
import com.lycoris.maps.core.network.requireSuccess
import com.lycoris.maps.core.network.requireUserData
import java.io.File
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.MultipartBody
import okhttp3.RequestBody.Companion.asRequestBody

// Origin, owner, and epoch form one scope; user ID alone cannot reject a stale response
// after logout/login or a service change.
data class SessionIdentity(val origin: String, val publicId: String, val epoch: Long)

data class AccountState(
    val initialized: Boolean = false,
    val user: User? = null,
    val epoch: Long = 0,
    val busy: Boolean = false,
    val failure: ApiFailure? = null,
    val favoriteIds: Set<Long> = emptySet(),
    val favoritesInitialized: Boolean = false,
    val favoritePlaces: List<Marker> = emptyList(),
    val createdPlaces: List<Marker> = emptyList(),
    val privateLanguage: Language? = null,
    val favoritesLoading: Boolean = false,
    val createdLoading: Boolean = false,
    val pendingFavoriteIds: Set<Long> = emptySet(),
)

/** Session transitions and writes share a gate; reads are cancelled on an epoch change. */
class AccountRepository(
    private val cookies: SessionCookieJar,
    private val apiForEpoch: (Long) -> LycorisApi,
    private val scope: CoroutineScope,
    private val io: CoroutineDispatcher = Dispatchers.IO,
) {
    constructor(clients: ApiClients, scope: CoroutineScope, io: CoroutineDispatcher = Dispatchers.IO) :
        this(clients.cookies, clients::authenticatedApi, scope, io)

    private val mutable = MutableStateFlow(AccountState(epoch = cookies.epoch()))
    val state: StateFlow<AccountState> = mutable.asStateFlow()
    private val gate = Mutex()
    private val sessionLock = Any()
    private var privateScope = newPrivateScope()
    private var favoritesRead = 0L
    private var createdRead = 0L
    private val inaccessiblePlaces = mutableSetOf<Long>()

    private fun newPrivateScope(): CoroutineScope = CoroutineScope(
        scope.coroutineContext + SupervisorJob(scope.coroutineContext[Job]),
    )

    fun identity(): SessionIdentity? = state.value.let { current ->
        current.user?.let { SessionIdentity(cookies.origin.toString(), it.publicId, current.epoch) }
    }

    /** A retained cookie after a failed restore is an unknown session, not proven anonymity. */
    internal suspend fun awaitReadyState(): AccountState {
        val ready = state.first { it.initialized && !it.busy }
        if (ready.user == null && cookies.hasCookies()) throw ready.failure ?: ApiFailure.SessionRequired()
        return ready
    }

    suspend fun restore() = withContext(io) {
        gate.withLock {
            if (state.value.initialized && state.value.user != null) return@withLock
            cookies.restore()
            val epoch = transition(clearCookies = false, initialized = false, busy = true)
            if (!cookies.hasCookies()) {
                mutable.update { it.copy(initialized = true, busy = false) }
                return@withLock
            }
            try {
                val user = apiCall { apiForEpoch(epoch).me().requireUserData().validUser() }
                publishUser(epoch, user)
            } catch (failure: ApiFailure) {
                if (failure is ApiFailure.Http && failure.status == 401) expire(epoch)
                else mutable.update { if (it.epoch == epoch) it.copy(initialized = true, failure = failure) else it }
            } finally {
                mutable.update { if (it.epoch == epoch) it.copy(busy = false, initialized = true) else it }
            }
        }
    }

    suspend fun login(username: String, password: String): User {
        if (username.isBlank() || password.isEmpty()) throw ApiFailure.InvalidInput("credentials")
        return authenticate { login(LoginRequest(username.trim(), password)).requireUserData() }
    }

    suspend fun register(username: String, nickname: String, email: String, password: String, verificationCode: String): User {
        validateRegistration(username, nickname, email, password)
        if (!verificationCode.matches(Regex("[0-9]{6}"))) throw ApiFailure.InvalidInput("verificationCode")
        return authenticate {
            register(RegisterRequest(username.trim(), nickname.trim(), email.trim().lowercase(), password, verificationCode)).requireUserData()
        }
    }

    suspend fun sendEmailCode(email: String, reset: Boolean, language: String) = withContext(io) {
        gate.withLock {
            apiCall { apiForEpoch(state.value.epoch).sendEmailCode(
                EmailCodeRequest(email.trim().lowercase(), if (reset) "reset_password" else "register"), language,
            ).requireEnvelope() }
        }
    }

    suspend fun resetPassword(email: String, code: String, password: String) = withContext(io) {
        if (!code.matches(Regex("[0-9]{6}"))) throw ApiFailure.InvalidInput("verificationCode")
        if (password.length < 4 || password.toByteArray(Charsets.UTF_8).size > 72) throw ApiFailure.InvalidInput("password")
        gate.withLock {
            apiCall { apiForEpoch(state.value.epoch).resetPassword(ResetPasswordRequest(email.trim().lowercase(), code, password)).requireEnvelope() }
            transition(clearCookies = true)
        }
    }

    private suspend fun authenticate(block: suspend LycorisApi.() -> User): User = withContext(io) {
        gate.withLock {
            val epoch = transition(clearCookies = true, busy = true)
            try {
                val user = apiCall { apiForEpoch(epoch).block().validUser() }
                publishUser(epoch, user)
                user
            } catch (failure: ApiFailure) {
                // Login's 401 means invalid credentials, not an additional session-expired event.
                if (state.value.epoch == epoch) {
                    transition(clearCookies = true, failure = failure)
                }
                throw failure
            } catch (cancelled: CancellationException) {
                if (state.value.epoch == epoch) transition(clearCookies = true)
                throw cancelled
            } finally {
                mutable.update { it.copy(busy = false, initialized = true) }
            }
        }
    }

    suspend fun logout() = withContext(io) {
        gate.withLock {
            val identity = identity()
            if (identity == null) {
                transition(clearCookies = true)
                return@withLock
            }
            mutable.update { it.copy(busy = true, failure = null) }
            try {
                apiCall { apiForEpoch(identity.epoch).logout().requireEnvelope() }
                transition(clearCookies = true)
            } catch (failure: ApiFailure) {
                if (failure is ApiFailure.Http && failure.status == 401) {
                    expire(identity.epoch)
                } else {
                    // A 503 is not proof of logout: preserve the identity and explain the error.
                    mutable.update { if (it.epoch == identity.epoch) it.copy(failure = failure) else it }
                    throw failure
                }
            } finally {
                mutable.update { it.copy(busy = false) }
            }
        }
    }

    /** Explicit local lock, distinct from successful server logout. */
    suspend fun clearLocalSession() = withContext(io) { gate.withLock { transition(clearCookies = true); Unit } }

    suspend fun updateProfile(nickname: String?, pronouns: String?, signature: String?, expectedIdentity: SessionIdentity? = null): User {
        if (nickname != null && nickname.scalarCount() > 255) throw ApiFailure.InvalidInput("nickname")
        if (pronouns != null && pronouns.scalarCount() > 64) throw ApiFailure.InvalidInput("pronouns")
        if (signature != null && signature.scalarCount() > 200) throw ApiFailure.InvalidInput("signature")
        return withAuthenticatedMutation(expectedIdentity = expectedIdentity) { api, identity ->
            val user = try {
                api.updateProfile(UpdateProfileRequest(nickname, pronouns, signature)).requireUserData().validUser()
            } catch (failure: ApiFailure.Http) {
                if (failure.status == 409) {
                    val fresh = api.me().requireUserData().validUser()
                    publishUser(identity.epoch, fresh)
                }
                throw failure
            }
            publishUser(identity.epoch, user)
            user
        }
    }

    suspend fun uploadAvatar(file: File, mime: String, expectedIdentity: SessionIdentity? = null): User {
        if (!file.isFile || file.length() !in 1..5L * 1024 * 1024 || mime !in setOf("image/jpeg", "image/png", "image/webp")) {
            throw ApiFailure.InvalidInput("avatar")
        }
        return withAuthenticatedMutation(expectedIdentity = expectedIdentity) { api, identity ->
            val part = MultipartBody.Part.createFormData("file", "avatar.${if (mime == "image/png") "png" else if (mime == "image/webp") "webp" else "jpg"}", file.asRequestBody(mime.toMediaType()))
            val user = api.uploadAvatar(part).requireUserData().validUser()
            publishUser(identity.epoch, user)
            user
        }
    }

    suspend fun changePassword(oldPassword: String, newPassword: String, expectedIdentity: SessionIdentity? = null) {
        validatePassword(newPassword)
        withAuthenticatedMutation(expectedIdentity = expectedIdentity) { api, identity ->
            api.changePassword(ChangePasswordRequest(oldPassword, newPassword)).requireEnvelope()
            try {
                val user = api.me().requireUserData().validUser()
                publishUser(identity.epoch, user)
            } catch (failure: ApiFailure.Http) {
                if (failure.status == 401) expire(identity.epoch) else throw failure
            }
        }
    }

    /** Also used by owner-bound contribution workers, once per POST/chunk, never around backoff. */
    suspend fun <T> withAuthenticatedMutation(
        expectedOwner: String? = null,
        expectedIdentity: SessionIdentity? = null,
        block: suspend (LycorisApi, SessionIdentity) -> T,
    ): T = withContext(io) {
        gate.withLock {
            val identity = identity() ?: throw ApiFailure.SessionRequired()
            if (expectedOwner != null && expectedOwner != identity.publicId) throw ApiFailure.SessionChanged()
            if (expectedIdentity != null && expectedIdentity != identity) throw ApiFailure.SessionChanged()
            mutable.update { it.copy(failure = null) }
            try {
                val value = apiCall { block(apiForEpoch(identity.epoch), identity) }
                ensureIdentity(identity)
                value
            } catch (failure: ApiFailure) {
                handleFailure(identity, failure)
                throw failure
            }
        }
    }

    /** Authenticated reads use the epoch's cancellable scope and never borrow the next user's jar. */
    suspend fun <T> withAuthenticatedRead(
        expectedIdentity: SessionIdentity? = null,
        block: suspend (LycorisApi, SessionIdentity) -> T,
    ): T {
        val (identity, readScope) = synchronized(sessionLock) {
            (identity() ?: throw ApiFailure.SessionRequired()) to privateScope
        }
        if (expectedIdentity != null && expectedIdentity != identity) throw ApiFailure.SessionChanged()
        val task = readScope.async {
            try {
                apiCall { block(apiForEpoch(identity.epoch), identity) }.also { ensureIdentity(identity) }
            } catch (failure: ApiFailure) {
                handleFailure(identity, failure)
                throw failure
            }
        }
        return try { task.await() } finally { task.cancel() }
    }

    suspend fun refreshFavorites(language: Language) {
        val epoch = identity()?.epoch ?: return
        val revision = synchronized(sessionLock) {
            if (state.value.epoch != epoch) return
            prepareLanguage(language)
            ++favoritesRead
        }
        mutable.update { if (it.epoch == epoch) it.copy(
            favoritesLoading = true, favoritesInitialized = false, failure = null,
            favoritePlaces = if (it.privateLanguage == language) it.favoritePlaces else emptyList(),
            createdPlaces = if (it.privateLanguage == language) it.createdPlaces else emptyList(),
            privateLanguage = language,
        ) else it }
        try {
            withAuthenticatedRead { api, _ ->
                val ids = api.favoriteIds().requireBody().filter { it > 0 }.toSet()
                val places = api.favoritePlaces(language.tag).requireBody().validMarkers().filter { it.id in ids }
                synchronized(sessionLock) {
                    if (favoritesRead == revision) mutable.update { if (it.epoch == epoch && it.privateLanguage == language && it.pendingFavoriteIds.isEmpty()) it.copy(
                        favoriteIds = ids - inaccessiblePlaces, favoritePlaces = places.filterNot { place -> place.id in inaccessiblePlaces }, favoritesInitialized = true,
                    ) else it }
                }
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: ApiFailure) {
            // Shared authenticated read publishes a typed error; existing same-language items remain.
            synchronized(sessionLock) {
                if (favoritesRead == revision) mutable.update { if (it.epoch == epoch) it.copy(favoritesInitialized = false) else it }
            }
        } finally {
            synchronized(sessionLock) {
                if (favoritesRead == revision) mutable.update { if (it.epoch == epoch) it.copy(favoritesLoading = false) else it }
            }
        }
    }

    /** Per-id guard plus the mutation gate prevents duplicate taps and query/write races. */
    suspend fun toggleFavorite(id: Long, language: Language) = changeFavorite(id, language, desired = null)

    /** Used for deferred "sign in to bookmark" intent; an existing bookmark must not be removed. */
    suspend fun setFavorite(id: Long, desired: Boolean, language: Language) = changeFavorite(id, language, desired)

    private suspend fun changeFavorite(id: Long, language: Language, desired: Boolean?) {
        if (id <= 0) throw ApiFailure.InvalidInput("marker")
        val epoch = identity()?.epoch ?: throw ApiFailure.SessionRequired()
        val accepted = synchronized(sessionLock) {
            if (state.value.epoch != epoch) throw ApiFailure.SessionChanged()
            if (id in state.value.pendingFavoriteIds) false else {
                prepareLanguage(language)
                favoritesRead++ // A pre-write snapshot may no longer publish.
                mutable.update { it.copy(pendingFavoriteIds = it.pendingFavoriteIds + id, favoritesLoading = false) }
                true
            }
        }
        if (!accepted) return
        try {
            withAuthenticatedMutation { api, identity ->
                if (identity.epoch != epoch) throw ApiFailure.SessionChanged()
                if (!state.value.favoritesInitialized) {
                    val initialIds = api.favoriteIds().requireBody().filter { it > 0 }.toSet()
                    synchronized(sessionLock) {
                        mutable.update { if (it.epoch == epoch) it.copy(favoriteIds = initialIds - inaccessiblePlaces) else it }
                    }
                }
                val wasFavorite = id in state.value.favoriteIds
                val shouldBeFavorite = desired ?: !wasFavorite
                if (wasFavorite != shouldBeFavorite) {
                    if (shouldBeFavorite) api.addFavorite(id).requireSuccess() else api.removeFavorite(id).requireSuccess()
                }
                mutable.update { if (it.epoch == epoch) it.copy(
                    favoriteIds = if (shouldBeFavorite) it.favoriteIds + id else it.favoriteIds - id,
                    favoritePlaces = if (shouldBeFavorite) it.favoritePlaces else it.favoritePlaces.filterNot { marker -> marker.id == id },
                ) else it }
                // The write already succeeded. Keep that fact even if this reconciliation GET fails.
                val ids = api.favoriteIds().requireBody().filter { it > 0 }.toSet()
                val places = api.favoritePlaces(language.tag).requireBody().validMarkers().filter { it.id in ids }
                synchronized(sessionLock) {
                    mutable.update { if (it.epoch == epoch) it.copy(
                        favoriteIds = ids - inaccessiblePlaces, favoritesInitialized = it.privateLanguage == language,
                        favoritePlaces = if (it.privateLanguage == language) places.filterNot { place -> place.id in inaccessiblePlaces } else it.favoritePlaces,
                    ) else it }
                }
            }
        } catch (failure: ApiFailure) {
            mutable.update { if (it.epoch == epoch) it.copy(favoritesInitialized = false) else it }
            throw failure
        } finally {
            synchronized(sessionLock) {
                favoritesRead++ // Reject a read that started during this write as well.
                mutable.update { if (it.epoch == epoch) it.copy(pendingFavoriteIds = it.pendingFavoriteIds - id, favoritesLoading = false) else it }
            }
        }
    }

    suspend fun refreshCreated(language: Language) {
        val epoch = identity()?.epoch ?: return
        val revision = synchronized(sessionLock) {
            if (state.value.epoch != epoch) return
            prepareLanguage(language)
            ++createdRead
        }
        mutable.update { if (it.epoch == epoch) it.copy(
            createdLoading = true, failure = null,
            createdPlaces = if (it.privateLanguage == language) it.createdPlaces else emptyList(),
            favoritePlaces = if (it.privateLanguage == language) it.favoritePlaces else emptyList(),
            privateLanguage = language,
        ) else it }
        try {
            withAuthenticatedRead { api, _ ->
                val places = api.createdPlaces(language.tag).requireBody().validMarkers()
                synchronized(sessionLock) {
                    if (createdRead == revision) mutable.update { if (it.epoch == epoch && it.privateLanguage == language) it.copy(createdPlaces = places.filterNot { place -> place.id in inaccessiblePlaces }) else it }
                }
            }
        } catch (_: ApiFailure) {
            // Preserve only the current account's last successful content.
        } finally {
            synchronized(sessionLock) {
                if (createdRead == revision) mutable.update { if (it.epoch == epoch) it.copy(createdLoading = false) else it }
            }
        }
    }

    fun dismissFailure() { mutable.update { it.copy(failure = null) } }

    fun prunePlace(id: Long) = synchronized(sessionLock) {
        inaccessiblePlaces += id
        mutable.update { it.copy(
            favoriteIds = it.favoriteIds - id,
            favoritePlaces = it.favoritePlaces.filterNot { marker -> marker.id == id },
            createdPlaces = it.createdPlaces.filterNot { marker -> marker.id == id },
        ) }
    }

    private fun prepareLanguage(language: Language) {
        if (state.value.privateLanguage != language) {
            favoritesRead++
            createdRead++
            mutable.update { it.copy(privateLanguage = language, favoritePlaces = emptyList(), createdPlaces = emptyList(), favoritesInitialized = false, favoritesLoading = false, createdLoading = false) }
        }
    }

    private fun ensureIdentity(expected: SessionIdentity) {
        if (identity() != expected) throw ApiFailure.SessionChanged()
    }

    private fun handleFailure(identity: SessionIdentity, failure: ApiFailure) {
        if (failure is ApiFailure.Http && failure.status == 401) expire(identity.epoch)
        else mutable.update { if (it.epoch == identity.epoch) it.copy(failure = failure) else it }
    }

    private fun publishUser(epoch: Long, user: User) {
        synchronized(sessionLock) {
            val previous = state.value
            if (previous.epoch != epoch) throw ApiFailure.SessionChanged()
            if (previous.user != null && previous.user.publicId != user.publicId) {
                // Even an unexpected server-side identity replacement must not keep private caches.
                transition(clearCookies = false)
                mutable.update { it.copy(user = user, initialized = true, busy = false) }
            } else mutable.update { it.copy(user = user, initialized = true, busy = false, failure = null) }
        }
    }

    private fun expire(epoch: Long) {
        synchronized(sessionLock) {
            if (state.value.epoch == epoch) transition(clearCookies = true, failure = ApiFailure.SessionRequired())
        }
    }

    private fun transition(
        clearCookies: Boolean,
        failure: ApiFailure? = null,
        initialized: Boolean = true,
        busy: Boolean = false,
    ): Long = synchronized(sessionLock) {
        privateScope.cancel()
        privateScope = newPrivateScope()
        favoritesRead++
        createdRead++
        inaccessiblePlaces.clear()
        var transitionFailure: ApiFailure? = null
        try {
            cookies.advanceEpoch(clearCookies)
        } catch (error: ApiFailure) {
            transitionFailure = error
            throw error
        } finally {
            mutable.value = AccountState(
                initialized = transitionFailure != null || initialized,
                busy = transitionFailure == null && busy,
                epoch = cookies.epoch(), failure = transitionFailure ?: failure,
            )
        }
        cookies.epoch()
    }
}

private fun User.validUser(): User = also { if (publicId.isBlank()) throw ApiFailure.InvalidResponse() }
private fun String.scalarCount(): Int = codePointCount(0, length)
private fun validatePassword(password: String) {
    if (password.length < 4 || password.toByteArray(Charsets.UTF_8).size > 72) throw ApiFailure.InvalidInput("password")
}
/**
 * Registration only. Sign-in matches a username or an email, so a new username shaped like an
 * address could shadow someone's email; existing usernames containing `@` still sign in.
 */
fun isRegistrableUsername(username: String): Boolean = '@' !in username
private fun validateRegistration(username: String, nickname: String, email: String, password: String) {
    if (username.isBlank() || username.trim().scalarCount() > 255) throw ApiFailure.InvalidInput("username")
    if (!isRegistrableUsername(username)) throw ApiFailure.InvalidInput("usernameAt")
    if (nickname.trim().scalarCount() > 255) throw ApiFailure.InvalidInput("nickname")
    if (email.isBlank() || email.trim().scalarCount() > 255) throw ApiFailure.InvalidInput("email")
    validatePassword(password)
}
