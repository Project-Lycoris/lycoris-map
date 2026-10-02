package com.lycoris.maps.core.data

import com.lycoris.maps.core.data.preferences.SearchType
import com.lycoris.maps.core.model.GeoBounds
import com.lycoris.maps.core.model.Language
import com.lycoris.maps.core.model.Marker
import com.lycoris.maps.core.model.PlaceCategory
import com.lycoris.maps.core.model.validCoordinate
import com.lycoris.maps.core.model.validMarkers
import com.lycoris.maps.core.network.ApiFailure
import com.lycoris.maps.core.network.LycorisApi
import com.lycoris.maps.core.network.apiCall
import com.lycoris.maps.core.network.requireBody
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

data class PlaceListState(
    val places: List<Marker> = emptyList(),
    val language: Language = Language.ZH,
    val loading: Boolean = false,
    val failure: ApiFailure? = null,
    val loaded: Boolean = false,
    val query: String = "",
)

data class PlaceDetailState(
    val id: Long? = null,
    val place: Marker? = null,
    val language: Language = Language.ZH,
    val loading: Boolean = false,
    val failure: ApiFailure? = null,
    val missing: Boolean = false,
)

/** Independent request channels keep search cancellation from interfering with the map. */
class PlaceRepository(
    private val publicApi: LycorisApi,
    private val scope: CoroutineScope,
    private val accounts: AccountRepository? = null,
) {
    private class Channel {
        val state = MutableStateFlow(PlaceListState())
        var generation = 0L
        var job: Job? = null
    }
    private val viewportChannel = Channel()
    private val searchChannel = Channel()
    private val nearbyChannel = Channel()
    val viewport: StateFlow<PlaceListState> = viewportChannel.state.asStateFlow()
    val search: StateFlow<PlaceListState> = searchChannel.state.asStateFlow()
    val nearby: StateFlow<PlaceListState> = nearbyChannel.state.asStateFlow()
    private val detailMutable = MutableStateFlow(PlaceDetailState())
    val detail: StateFlow<PlaceDetailState> = detailMutable.asStateFlow()
    private val lock = Any()
    private val inaccessible = mutableSetOf<Long>()
    private var detailGeneration = 0L
    private var detailJob: Job? = null
    private var observedEpoch = accounts?.state?.value?.epoch

    init {
        if (accounts != null) scope.launch {
            accounts.state.map { it.epoch }.distinctUntilChanged().collect { epoch ->
                synchronized(lock) {
                    if (observedEpoch != epoch) {
                        observedEpoch = epoch
                        detailGeneration++
                        detailJob?.cancel()
                        // Public viewport/search data is identity independent. A private detail is not.
                        detailMutable.update { if (it.place?.publiclyVisible == true) it.copy(loading = false) else PlaceDetailState(language = it.language) }
                        inaccessible.clear() // A new account may legitimately see a previously hidden place.
                    }
                }
            }
        }
    }

    fun loadViewport(bounds: GeoBounds, language: Language, categories: Set<PlaceCategory> = emptySet()): Job =
        requestList(viewportChannel, "${bounds}|${categories.sortedBy { it.wireValue }}", language, retainSameLanguage = true) {
            coroutineScope {
                bounds.segments().map { segment -> async {
                    publicApi.viewport(
                        segment.south, segment.north, segment.west, segment.east,
                        categories.takeIf { it.isNotEmpty() }?.map { it.wireValue }?.sorted()?.joinToString(","), language.tag,
                    ).requireBody()
                } }.awaitAll().flatten()
            }
        }

    fun search(query: String, language: Language, debounceMillis: Long = 300): Job =
        requestList(searchChannel, query.trim(), language, debounceMillis = debounceMillis) {
            if (query.isBlank()) emptyList() else publicApi.search(query.trim(), language.tag).requireBody()
        }

    // Search returns the complete result set. Keep it intact so changing the preference
    // re-filters existing and in-flight results without new requests or affecting Nearby.
    fun searchResults(types: Flow<SearchType>): Flow<PlaceListState> = combine(search, types) { result, type ->
        val category = type.category
        if (category == null) result else result.copy(places = result.places.filter { category.wireValue in it.facilityCategories })
    }

    fun loadNearby(lat: Double, lng: Double, radiusMeters: Int, category: PlaceCategory, language: Language): Job =
        requestList(nearbyChannel, "$lat|$lng|$radiusMeters|${category.wireValue}", language) {
            if (!validCoordinate(lat, lng)) throw ApiFailure.InvalidInput("coordinate")
            if (radiusMeters !in 1..50000) throw ApiFailure.InvalidInput("radius")
            publicApi.nearby(lat, lng, radiusMeters, category.wireValue, language.tag).requireBody()
        }

    fun loadDetail(id: Long, language: Language): Job = synchronized(lock) {
        detailJob?.cancel()
        val generation = ++detailGeneration
        val previous = detailMutable.value
        detailMutable.value = PlaceDetailState(
            id = id, language = language, loading = true,
            place = previous.place.takeIf { previous.id == id && previous.language == language },
        )
        scope.launch {
            var requestEpoch: Long? = null
            var detailRequestStarted = false
            try {
                if (id <= 0) throw ApiFailure.InvalidInput("marker")
                // A cold private deep link must not issue an anonymous 404 while /me is restoring.
                val ready = accounts?.awaitReadyState()
                val epoch = ready?.epoch
                requestEpoch = epoch
                val identity = if (ready?.user != null) accounts?.identity()?.takeIf {
                    it.epoch == epoch && it.publicId == ready.user.publicId
                } ?: throw ApiFailure.SessionChanged() else null
                detailRequestStarted = true
                val place = apiCall {
                    if (identity != null) accounts!!.withAuthenticatedRead(expectedIdentity = identity) { api, _ -> api.marker(id, language.tag).requireBody() }
                    else publicApi.marker(id, language.tag).requireBody()
                }
                if (!place.hasValidLocation || place.id != id || place.deactivated) throw ApiFailure.InvalidResponse()
                synchronized(lock) {
                    if (generation == detailGeneration && epoch == accounts?.state?.value?.epoch) {
                        inaccessible.remove(id)
                        detailMutable.value = PlaceDetailState(id, place, language)
                    }
                }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: ApiFailure) {
                synchronized(lock) {
                    if (generation == detailGeneration && (!detailRequestStarted || requestEpoch == accounts?.state?.value?.epoch)) {
                        val missing = detailRequestStarted && failure is ApiFailure.Http && failure.status == 404
                        if (missing) pruneLocked(id)
                        detailMutable.update { it.copy(place = if (missing) null else it.place, loading = false, failure = failure, missing = missing) }
                    }
                }
            } finally {
                synchronized(lock) {
                    if (generation == detailGeneration) detailMutable.update { it.copy(loading = false) }
                }
            }
        }.also { detailJob = it }
    }

    fun closeDetail() = synchronized(lock) {
        detailGeneration++
        detailJob?.cancel()
        detailMutable.value = PlaceDetailState(language = detailMutable.value.language)
    }

    fun cancelSearch() = synchronized(lock) {
        searchChannel.generation++
        searchChannel.job?.cancel()
        searchChannel.state.update { it.copy(loading = false) }
    }

    fun prune(id: Long) = synchronized(lock) { pruneLocked(id) }

    private fun pruneLocked(id: Long) {
        inaccessible += id
        accounts?.prunePlace(id)
        for (channel in listOf(viewportChannel, searchChannel, nearbyChannel)) {
            channel.state.update { it.copy(places = it.places.filterNot { place -> place.id == id }) }
        }
        detailMutable.update { if (it.id == id) it.copy(place = null, missing = true) else it }
    }

    private fun requestList(
        channel: Channel,
        query: String,
        language: Language,
        retainSameLanguage: Boolean = false,
        debounceMillis: Long = 0,
        load: suspend () -> List<Marker>,
    ): Job = synchronized(lock) {
        channel.job?.cancel()
        val generation = ++channel.generation
        val previous = channel.state.value
        val retain = previous.language == language && (retainSameLanguage || previous.query == query)
        channel.state.value = PlaceListState(
            places = if (retain) previous.places else emptyList(),
            language = language, loading = true, loaded = retain && previous.loaded, query = query,
        )
        scope.launch {
            try {
                if (debounceMillis > 0) delay(debounceMillis)
                val result = apiCall { load() }.validMarkers().filter { it.publiclyVisible }
                synchronized(lock) {
                    if (generation == channel.generation) channel.state.value = PlaceListState(
                        result.filterNot { it.id in inaccessible }, language, loaded = true, query = query,
                    )
                }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: ApiFailure) {
                synchronized(lock) {
                    if (generation == channel.generation) channel.state.update { it.copy(loading = false, failure = failure) }
                }
            } finally {
                synchronized(lock) {
                    if (generation == channel.generation) channel.state.update { it.copy(loading = false) }
                }
            }
        }.also { channel.job = it }
    }
}
