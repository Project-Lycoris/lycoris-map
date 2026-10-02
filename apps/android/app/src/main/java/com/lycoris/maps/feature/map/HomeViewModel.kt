package com.lycoris.maps.feature.map

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.viewModelScope
import com.lycoris.maps.app.LycorisApplication
import com.lycoris.maps.core.data.PlaceRepository
import com.lycoris.maps.core.data.PlaceListState
import com.lycoris.maps.core.data.preferences.SearchType
import com.lycoris.maps.core.map.MapBounds
import com.lycoris.maps.core.map.MapCamera
import com.lycoris.maps.core.map.ViewportPolicy
import com.lycoris.maps.core.model.GeoBounds
import com.lycoris.maps.core.model.Language
import com.lycoris.maps.core.model.PlaceCategory
import com.lycoris.maps.core.network.requireBody
import com.lycoris.maps.core.network.ApiFailure
import com.lycoris.maps.core.model.Marker
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import java.io.IOException
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch

enum class SecondaryPage { SEARCH, NEARBY, DETAIL, ACCOUNT, CREATED, CONTRIBUTION }

class HomeViewModel(application: Application, private val saved: SavedStateHandle) : AndroidViewModel(application) {
    val container = (application as LycorisApplication).container
    val preferences = container.preferences.state
    val accounts = container.accounts
    val places = PlaceRepository(container.clients.publicApi, viewModelScope, accounts)
    val search = places.searchResults(preferences.map { it.searchType }.distinctUntilChanged())
        .stateIn(viewModelScope, SharingStarted.Eagerly, PlaceListState())
    val section = saved.getStateFlow("section", MainSection.EXPLORE)
    val page = saved.getStateFlow<SecondaryPage?>("page", null)
    val query = saved.getStateFlow("query", "")
    val nearbyCategory = saved.getStateFlow("nearby", PlaceCategory.ACCESSIBLE_TOILET)
    private val notices = AppNotices()
    val notice: StateFlow<String?> = notices.state
    private val viewportPolicy = ViewportPolicy()
    private var camera = MapCamera()
    private var bounds: MapBounds? = null
    val allowInitialCenter = saved.getStateFlow("allowInitialCenter", true)
    val pickingLocation = saved.getStateFlow("pickingLocation", false)
    val draftId = saved.getStateFlow<String?>("draftId", null)
    private var viewportJob: Job? = null
    private var creatingDraft = false
    private var navigationGeneration = 0L
    private var pendingEditLookup: Job? = null

    init {
        viewModelScope.launch {
            var restoring = !accounts.state.value.initialized
            var firstReady = true
            var observedIdentity: Pair<Long, String?>? = null
            accounts.state.collect { account ->
                    if (!account.initialized) { restoring = true; return@collect }
                    if (account.busy) return@collect
                    val user = account.user?.publicId
                    val restored = restoring
                    restoring = false
                    // Passive /me restoration has no login-form completion callback.
                    if ((restored || firstReady) && user != null && page.value == SecondaryPage.ACCOUNT && hasPendingAccountAction()) signedIn()
                    firstReady = false
                    val identity = account.epoch to user
                    if (identity == observedIdentity) return@collect
                    observedIdentity = identity
                    if (user != null) viewModelScope.launch { accounts.refreshFavorites(preferences.value.language) }
                    val current = page.value
                    val returning = saved.get<SecondaryPage>("returnPage")
                    if (current == SecondaryPage.DETAIL || returning == SecondaryPage.DETAIL) {
                        saved.get<Long>("markerId")?.let { places.loadDetail(it, preferences.value.language) }
                    }
                    if (user != null && current == SecondaryPage.CREATED) viewModelScope.launch { accounts.refreshCreated(preferences.value.language) }
                }
        }
        viewModelScope.launch {
            preferences.map { it.language to it.radiusMeters }.distinctUntilChanged().collect {
                if (page.value == SecondaryPage.NEARBY) nearby(null)
            }
        }
        viewModelScope.launch {
            preferences.map { it.language }.distinctUntilChanged().collect {
                viewportPolicy.invalidate()
                bounds?.let { current -> cameraIdle(camera, current) }
                if (query.value.isNotBlank()) places.search(query.value, it)
                saved.get<Long>("markerId")?.let { id -> if (page.value == SecondaryPage.DETAIL) places.loadDetail(id, it) }
                if (accounts.state.value.user != null) {
                    viewModelScope.launch { accounts.refreshFavorites(it) }
                    if (page.value == SecondaryPage.CREATED) viewModelScope.launch { accounts.refreshCreated(it) }
                }
            }
        }
        if (page.value == SecondaryPage.DETAIL) saved.get<Long>("markerId")?.let { places.loadDetail(it, preferences.value.language) }
        if (page.value == SecondaryPage.SEARCH) places.search(query.value, preferences.value.language)
    }

    fun cameraIdle(value: MapCamera, visible: MapBounds) {
        camera = value; bounds = visible
        val safe = runCatching { GeoBounds(visible.south.coerceIn(-90.0, 90.0), visible.north.coerceIn(-90.0, 90.0), visible.west.coerceIn(-180.0, 180.0), visible.east.coerceIn(-180.0, 180.0)) }.getOrNull() ?: return
        val request = viewportPolicy.request(safe, value.zoom, preferences.value.language) ?: return
        viewportJob?.cancel()
        viewportJob = viewModelScope.launch {
            delay(180)
            places.loadViewport(request.bounds, request.language).join()
            viewportPolicy.complete(request, places.viewport.value.failure == null && places.viewport.value.loaded)
        }
    }
    fun mapGesture() { saved["allowInitialCenter"] = false }
    fun retryViewport() { viewportPolicy.invalidate(); bounds?.let { cameraIdle(camera, it) } }
    private fun beginNavigation(): Long {
        pendingEditLookup?.cancel()
        pendingEditLookup = null
        return ++navigationGeneration
    }
    private fun hasPendingAccountAction(): Boolean = saved.get<Long>("pendingFavorite") != null ||
        saved.get<Boolean>("pendingContribution") == true || saved.get<Long>("pendingEdit") != null
    private fun abandonPendingAccountAction() {
        saved["pendingFavorite"] = null; saved["pendingContribution"] = false; saved["pendingEdit"] = null; saved["returnPage"] = null
    }
    private fun leaveAccountForNavigation() {
        if (page.value == SecondaryPage.ACCOUNT) abandonPendingAccountAction()
    }
    fun selectSection(value: MainSection) {
        beginNavigation()
        abandonPendingAccountAction(); saved["page"] = null; saved["pickingLocation"] = false
        places.closeDetail(); saved["section"] = value
        if (value == MainSection.BOOKMARKS && accounts.state.value.user != null) viewModelScope.launch { accounts.refreshFavorites(preferences.value.language) }
    }
    fun setQuery(value: String) {
        beginNavigation(); saved["pickingLocation"] = false
        saved["query"] = value
        if (value.isNotBlank()) { leaveAccountForNavigation(); saved["page"] = SecondaryPage.SEARCH; places.search(value, preferences.value.language) }
        else if (page.value == SecondaryPage.SEARCH) closeSecondary()
    }
    fun search() { beginNavigation(); leaveAccountForNavigation(); saved["pickingLocation"] = false; saved["page"] = SecondaryPage.SEARCH; places.search(query.value, preferences.value.language, 0) }
    fun nearby(key: String?, latitude: Double? = null, longitude: Double? = null) {
        beginNavigation()
        leaveAccountForNavigation(); saved["pickingLocation"] = false
        val category = key?.let(PlaceCategory::fromWire) ?: nearbyCategory.value
        val lat = latitude ?: if (page.value == SecondaryPage.NEARBY) saved.get<Double>("nearbyLat") ?: camera.latitude else camera.latitude
        val lng = longitude ?: if (page.value == SecondaryPage.NEARBY) saved.get<Double>("nearbyLng") ?: camera.longitude else camera.longitude
        saved["nearbyLat"] = lat; saved["nearbyLng"] = lng
        saved["nearby"] = category; saved["page"] = SecondaryPage.NEARBY
        places.loadNearby(lat, lng, preferences.value.radiusMeters, category, preferences.value.language)
    }
    fun detail(id: Long) { beginNavigation(); leaveAccountForNavigation(); saved["pickingLocation"] = false; saved["allowInitialCenter"] = false; saved["page"] = SecondaryPage.DETAIL; saved["markerId"] = id; places.loadDetail(id, preferences.value.language) }
    fun account() {
        beginNavigation(); saved["pickingLocation"] = false
        if (page.value != SecondaryPage.ACCOUNT) saved["returnPage"] = page.value
        saved["page"] = SecondaryPage.ACCOUNT
    }
    fun favorite(id: Long) {
        if (accounts.state.value.user == null) { saved["pendingFavorite"] = id; account(); return }
        action { accounts.toggleFavorite(id, preferences.value.language) }
    }
    fun contribute() {
        beginNavigation()
        leaveAccountForNavigation(); saved["allowInitialCenter"] = false
        if (accounts.state.value.user == null) { saved["pendingContribution"] = true; account() }
        else { saved["page"] = null; saved["pickingLocation"] = true }
    }
    fun cancelPicking() { beginNavigation(); saved["pickingLocation"] = false }
    fun pickLocation(latitude: Double, longitude: Double) {
        if (!pickingLocation.value || creatingDraft) return
        val navigation = beginNavigation()
        creatingDraft = true
        action {
            try {
                val draft = container.contributions.createDraft(latitude, longitude, preferences.value.language.tag)
                if (navigationGeneration == navigation && pickingLocation.value) {
                    saved["draftId"] = draft
                    saved["pickingLocation"] = false
                    saved["page"] = SecondaryPage.CONTRIBUTION
                }
            } finally { creatingDraft = false }
        }
    }
    fun edit(place: Marker) {
        if (accounts.state.value.user == null) { saved["pendingEdit"] = place.id; account(); return }
        val navigation = beginNavigation()
        action {
            val source = accounts.withAuthenticatedRead { api, _ -> api.marker(place.id, "zh").requireBody() }
            if (navigationGeneration != navigation) return@action
            val draft = container.contributions.createDraft(source.lat, source.lng, "zh", source)
            if (navigationGeneration == navigation) {
                saved["draftId"] = draft
                saved["page"] = SecondaryPage.CONTRIBUTION
            }
        }
    }
    fun openDraft(id: String) { beginNavigation(); saved["draftId"] = id; saved["page"] = SecondaryPage.CONTRIBUTION }
    fun signedIn() {
        if (page.value != SecondaryPage.ACCOUNT) return
        val navigation = beginNavigation()
        saved["page"] = saved.get<SecondaryPage>("returnPage"); saved["returnPage"] = null
        val favorite = saved.get<Long>("pendingFavorite"); saved["pendingFavorite"] = null
        val contribute = saved.get<Boolean>("pendingContribution") == true; saved["pendingContribution"] = false
        val edit = saved.get<Long>("pendingEdit"); saved["pendingEdit"] = null
        reloadPage(page.value)
        favorite?.let { id -> action { accounts.setFavorite(id, desired = true, language = preferences.value.language) } }
        if (contribute) this.contribute()
        if (edit != null) pendingEditLookup = actionJob {
            places.loadDetail(edit, preferences.value.language)
            // Identity/language observers may replace this read. Follow the current result;
            // a new user navigation cancels this continuation without deleting any draft.
            val detail = places.detail.first { it.id == edit && !it.loading }
            if (navigationGeneration == navigation) {
                pendingEditLookup = null
                detail.place?.takeIf { detail.failure == null }?.let { this@HomeViewModel.edit(it) }
            }
        }
        if (section.value == MainSection.BOOKMARKS) viewModelScope.launch { accounts.refreshFavorites(preferences.value.language) }
    }
    fun myPlaces() { beginNavigation(); saved["page"] = SecondaryPage.CREATED; viewModelScope.launch { accounts.refreshCreated(preferences.value.language) } }
    fun closeSecondary() {
        beginNavigation()
        val previous = if (page.value == SecondaryPage.ACCOUNT) saved.get<SecondaryPage>("returnPage") else null
        saved["page"] = previous
        if (previous != SecondaryPage.DETAIL) places.closeDetail()
        saved["pendingFavorite"] = null; saved["pendingContribution"] = false; saved["pendingEdit"] = null; saved["returnPage"] = null
        reloadPage(previous)
    }
    private fun reloadPage(target: SecondaryPage?) {
        val language = preferences.value.language
        when (target) {
            SecondaryPage.SEARCH -> places.search(query.value, language, 0)
            SecondaryPage.NEARBY -> places.loadNearby(
                saved.get<Double>("nearbyLat") ?: camera.latitude,
                saved.get<Double>("nearbyLng") ?: camera.longitude,
                preferences.value.radiusMeters, nearbyCategory.value, language,
            )
            SecondaryPage.DETAIL -> saved.get<Long>("markerId")?.let { places.loadDetail(it, language) }
            SecondaryPage.CREATED -> if (accounts.state.value.user != null) viewModelScope.launch { accounts.refreshCreated(language) }
            else -> Unit
        }
    }
    fun draftCommand(block: suspend () -> Unit) = action {
        try { block() } catch (cancelled: CancellationException) { throw cancelled }
        catch (failure: ApiFailure) { throw failure }
        catch (_: Exception) { message(if (preferences.value.language == Language.ZH) "无法保存或提交，请检查网络及本机存储后重试。" else "Could not save or submit. Check your connection and device storage, then retry.") }
    }
    fun message(value: String?) { notices.showAction(value) }
    fun backgroundMessage(value: String?) { notices.showBackground(value) }
    fun language(value: Language) = action { container.preferences.setLanguage(value) }
    fun searchType(value: SearchType) = action { container.preferences.setSearchType(value) }
    fun acceptTencentPrivacy() = action { container.preferences.acceptTencentPrivacy() }
    fun mapSource(value: com.lycoris.maps.core.data.preferences.MapSource) = action { container.preferences.setMapSource(value) }
    fun radius(value: Int) = action { container.preferences.setRadius(value) }
    private fun action(block: suspend () -> Unit) { actionJob(block) }
    private fun actionJob(block: suspend () -> Unit): Job = viewModelScope.launch {
        try { block() } catch (cancelled: CancellationException) { throw cancelled }
        catch (failure: ApiFailure) { message(failure.displayMessage(preferences.value.language)) }
        catch (_: IOException) { message(if (preferences.value.language == Language.ZH) "无法保存，请重试。" else "Could not save. Please try again.") }
        catch (_: com.lycoris.maps.feature.contributions.DraftStorageFailure) { message(if (preferences.value.language == Language.ZH) "无法保存草稿，请重试。" else "Could not save your draft. Please try again.") }
    }
}

fun ApiFailure.displayMessage(language: Language): String {
    val zh = language == Language.ZH
    return when (this) {
        is ApiFailure.SessionRequired, is ApiFailure.SessionChanged -> if (zh) "请重新登录后再试。" else "Please sign in and try again."
        is ApiFailure.Network -> if (timedOut) { if (zh) "点位加载超时，请重试。" else "Loading places timed out. Try again." }
            else { if (zh) "网络不可用，请检查连接后重试。" else "Network unavailable. Check your connection and try again." }
        is ApiFailure.InvalidResponse -> if (zh) "点位数据无法解析，请重试。" else "The place data could not be read. Try again."
        is ApiFailure.Http -> when (status) {
            401 -> if (zh) "登录已过期，或账号密码不正确。" else "Please check your sign-in details or sign in again."
            403 -> if (zh) "当前账号无权查看这些点位。" else "You do not have permission to view these places."
            408, 504 -> if (zh) "点位加载超时，请重试。" else "Loading places timed out. Try again."
            in 500..599 -> if (zh) "点位服务暂时不可用，请稍后重试。" else "The place service is temporarily unavailable. Try again later."
            404 -> if (zh) "这个点位已不可用。" else "This place is no longer available."
            429 -> if (zh) "请求过于频繁，请稍后再试。" else "Please wait a moment before trying again."
            else -> if (zh) "暂时无法完成，请稍后重试。" else "Could not complete the request. Please try again."
        }
        is ApiFailure.InvalidInput -> if (zh) "请检查填写的内容。" else "Please check the entered information."
        else -> if (zh) "暂时无法完成，请重试。" else "Something went wrong. Please try again."
    }
}
