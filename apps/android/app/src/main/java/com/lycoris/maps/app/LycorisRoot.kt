package com.lycoris.maps.app

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.lycoris.maps.core.data.PlaceListState
import com.lycoris.maps.core.data.preferences.MapSource
import com.lycoris.maps.core.map.*
import com.lycoris.maps.core.model.Language
import com.lycoris.maps.core.platform.*
import com.lycoris.maps.feature.account.*
import com.lycoris.maps.feature.map.*
import com.lycoris.maps.feature.places.*
import com.lycoris.maps.feature.settings.SettingsDialog
import com.lycoris.maps.feature.settings.TencentPrivacyDialog
import com.lycoris.maps.feature.contributions.ContributionPanel

@Composable
fun LycorisRoot(model: HomeViewModel) {
    val context = LocalContext.current
    val map = rememberNativeMapState()
    val preferences by model.preferences.collectAsStateWithLifecycle()
    val account by model.accounts.state.collectAsStateWithLifecycle()
    val section by model.section.collectAsStateWithLifecycle()
    val page by model.page.collectAsStateWithLifecycle()
    val query by model.query.collectAsStateWithLifecycle()
    val notice by model.notice.collectAsStateWithLifecycle()
    val viewport by model.places.viewport.collectAsStateWithLifecycle()
    val search by model.search.collectAsStateWithLifecycle()
    val nearby by model.places.nearby.collectAsStateWithLifecycle()
    val nearbyCategory by model.nearbyCategory.collectAsStateWithLifecycle()
    val detail by model.places.detail.collectAsStateWithLifecycle()
    val allowInitialCenter by model.allowInitialCenter.collectAsStateWithLifecycle()
    val picking by model.pickingLocation.collectAsStateWithLifecycle()
    val draftId by model.draftId.collectAsStateWithLifecycle()
    val drafts by model.container.contributions.drafts.collectAsStateWithLifecycle()
    val renderedPlaces = remember(viewport.places, detail.place, page) {
        (viewport.places + listOfNotNull(detail.place?.takeIf { page == SecondaryPage.DETAIL })).distinctBy { it.id }
    }
    val zh = preferences.language == Language.ZH
    val needsTencentPrivacy = preferences.initialized && preferences.mapSource == MapSource.TENCENT && !preferences.tencentPrivacyAccepted
    val renderSource = if (needsTencentPrivacy) MapSource.OSM else preferences.mapSource
    val devices = rememberDeviceActions(map, preferences.language, { model.setQuery(it); model.search() }, model::message,
        allowInitialCenter = allowInitialCenter, onBackgroundMessage = model::backgroundMessage,
        onLocalNetworkGranted = model::retryViewport)
    var setting by rememberSaveable { mutableStateOf<String?>(null) }
    var accountPage by rememberSaveable { mutableStateOf(AccountPage.LOGIN) }
    var centeredDetailId by rememberSaveable { mutableStateOf<Long?>(null) }
    val navigation = remember(context) { NavigationLauncher(context) }
    var webFallback by remember { mutableStateOf<NavigationDestination?>(null) }
    var previousPage by rememberSaveable { mutableStateOf(page) }
    // Restoration may have finished before this composition (for example after process recreation).
    var awaitingAccountRestore by remember { mutableStateOf(true) }
    LaunchedEffect(page, account.initialized) {
        val restored = awaitingAccountRestore && account.initialized
        if (!account.initialized) awaitingAccountRestore = true
        else awaitingAccountRestore = false
        if (page == SecondaryPage.ACCOUNT && previousPage != SecondaryPage.ACCOUNT) accountPage = if (account.user == null) AccountPage.LOGIN else AccountPage.PROFILE
        else if (restored && page == SecondaryPage.ACCOUNT && account.user != null && accountPage == AccountPage.LOGIN) accountPage = AccountPage.PROFILE
        previousPage = page
    }
    LaunchedEffect(page) { devices.cancelVoice() }
    LaunchedEffect(page, viewport.failure, detail.failure, search.failure, nearby.failure, preferences.language) {
        val failure = when (page) {
            SecondaryPage.DETAIL -> detail.failure
            SecondaryPage.NEARBY -> nearby.failure
            SecondaryPage.SEARCH -> search.failure
            else -> null
        } ?: viewport.failure
        model.backgroundMessage(failure?.displayMessage(preferences.language))
    }
    LaunchedEffect(detail.place?.id, page, map.ready) {
        if (page != SecondaryPage.DETAIL) {
            centeredDetailId = null
        } else if (map.ready) {
            detail.place?.takeIf { it.id != centeredDetailId }?.let {
                centeredDetailId = it.id
                map.moveTo(it.lat, it.lng, map.camera.zoom.coerceAtLeast(15.0))
            }
        }
    }
    fun close() {
        devices.cancelVoice()
        if (page == SecondaryPage.ACCOUNT && accountPage in setOf(AccountPage.EDIT_PROFILE, AccountPage.PASSWORD)) accountPage = AccountPage.PROFILE
        else if (page == SecondaryPage.ACCOUNT && accountPage in setOf(AccountPage.REGISTER, AccountPage.RESET)) accountPage = AccountPage.LOGIN
        else model.closeSecondary()
    }
    BackHandler(picking) { model.cancelPicking() }
    val title = when (page) {
        SecondaryPage.SEARCH -> if (zh) "搜索结果" else "Search results"
        SecondaryPage.NEARBY -> if (zh) "附近点位" else "Nearby"
        SecondaryPage.DETAIL -> detail.place?.title ?: if (zh) "点位详情" else "Place details"
        SecondaryPage.ACCOUNT -> if (!account.initialized) { if (zh) "账号" else "Account" } else accountPage.title(preferences.language)
        SecondaryPage.CREATED -> if (zh) "我的点位" else "My places"
        SecondaryPage.CONTRIBUTION -> if (zh) "贡献点位" else "Contribute"
        null -> null
    }
    PlaceImageScope(model.container.clients, account) {
    HomeScreen(map, zh, section, model::selectSection, query, model::setQuery, model::search, devices.onVoice,
        { SearchAccountAvatar(account, model.container.clients, preferences.language) }, model::account, devices.onLocate,
        { category ->
            val fix = devices.location.fix
            model.nearby(category, fix?.latitude ?: map.camera.latitude, fix?.longitude ?: map.camera.longitude)
        }, model::contribute, { setting = "source" }, { setting = it }, preferences.radiusMeters, preferences.mapSource.title(preferences.language),
        mapSource = if (preferences.initialized) renderSource else MapSource.OSM,
        searchType = preferences.searchType,
        secondaryTitle = title, secondaryKey = page?.let { if (it == SecondaryPage.DETAIL) "detail:${detail.id}" else if (it == SecondaryPage.ACCOUNT) "account:$accountPage" else it.name }, onCloseSecondary = model::closeSecondary, onBackSecondary = ::close,
        notice = if (picking) { if (zh) "正在标注模式，请在地图上点击位置。" else "Tap the map to choose a place." } else notice,
        onDismissNotice = { if (picking) model.cancelPicking() else model.message(null) },
        onAttribution = {
            val url = if (renderSource == MapSource.TIANDITU) "https://www.tianditu.gov.cn/" else "https://www.openstreetmap.org/copyright"
            runCatching { context.startActivity(android.content.Intent(android.content.Intent.ACTION_VIEW, android.net.Uri.parse(url))) }
                .onFailure { model.message(if (zh) "没有可用的浏览器。" else "No browser is available.") }
        },
        onUserGesture = model::mapGesture, onCameraIdle = model::cameraIdle, onMapClick = { lat, lng -> model.pickLocation(lat, lng) },
        mapLayers = {
            if (renderSource == MapSource.GOOGLE) GooglePlaceLayers(map, renderedPlaces,
                { if (!picking) model.detail(it) }, devices.location.fix, devices.heading,
                onPickCoordinate = if (picking) model::pickLocation else null)
            else if (renderSource == MapSource.TENCENT && preferences.initialized) TencentPlaceLayers(map, renderedPlaces,
                { if (!picking) model.detail(it) }, devices.location.fix, devices.heading,
                onPickCoordinate = if (picking) model::pickLocation else null)
            else PlaceLayers(map, renderedPlaces, { if (!picking) model.detail(it) }, devices.location.fix, devices.heading)
        },
        panelContent = {
            when (page) {
                SecondaryPage.SEARCH -> placeItems(search, model.container.clients, account, zh, model::detail, model::search)
                SecondaryPage.NEARBY -> {
                    item(key = "nearby-subtitle", contentType = "summary") {
                        Column(Modifier.padding(horizontal = 30.dp).padding(bottom = 11.dp), verticalArrangement = Arrangement.spacedBy(11.dp)) {
                            val radiusText = if (preferences.radiusMeters % 1000 == 0) "${preferences.radiusMeters / 1000}km" else "${preferences.radiusMeters}m"
                            val categoryLabel = nearbyCategories.firstOrNull { it.key == nearbyCategory.wireValue }
                                ?.let { if (zh) it.zh else it.en } ?: categoryName(nearbyCategory.wireValue, zh)
                            Text("$categoryLabel · $radiusText", style = MaterialTheme.typography.bodyMedium)
                            if (devices.location.fix == null) Text(
                                if (zh) "以地图中心查找附近" else "Searching near the map center",
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                    }
                    placeItems(nearby, model.container.clients, account, zh, model::detail, { model.nearby(null) })
                }
                SecondaryPage.DETAIL -> item(key = "place-detail", contentType = "detail") { Column {
                    if (detail.loading) LinearProgressIndicator(Modifier.fillMaxWidth().padding(horizontal = 30.dp))
                    detail.failure?.let { failure ->
                        Text(failure.displayMessage(preferences.language), Modifier.padding(horizontal = 30.dp))
                        TextButton({ detail.id?.let(model::detail) }, Modifier.padding(horizontal = 30.dp)) { Text(if (zh) "重试" else "Retry") }
                    }
                    if (detail.missing) Text(if (zh) "这个点位已不可用。" else "This place is no longer available.", Modifier.padding(horizontal = 30.dp))
                    detail.place?.let { place ->
                        val fix = devices.location.fix
                        PlaceDetailContent(place, model.container.clients, account, zh, fix?.latitude ?: map.camera.latitude, fix?.longitude ?: map.camera.longitude,
                            onFavorite = { model.favorite(place.id) },
                            onNavigate = {
                                val destination = NavigationDestination(place.id, place.title, place.lat, place.lng)
                                if (navigation.showChooser(context, destination, if (zh) "选择导航应用" else "Choose a navigation app") is ExternalActionResult.NoNavigationApp) webFallback = destination
                            }, onShare = { if (PlaceSharing.share(context, place.id, place.title, preferences.language.tag) == ExternalActionResult.Unavailable) model.message(if (zh) "没有可用的分享应用。" else "No sharing app is available.") },
                            onEdit = { model.edit(place) })
                    }
                } }
                SecondaryPage.ACCOUNT -> item(key = "account-content", contentType = "account") {
                    if (!account.initialized) Column(Modifier.fillMaxWidth().padding(horizontal = 30.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                        LinearProgressIndicator(Modifier.fillMaxWidth())
                        Text(if (zh) "正在恢复登录状态…" else "Restoring your session…")
                    } else AccountPanel(model.accounts, model.container.clients, preferences.language, accountPage, { accountPage = it }, model::signedIn, model::closeSecondary, model::myPlaces)
                }
                SecondaryPage.CREATED -> {
                    item(key = "drafts-heading") { Text(if (zh) "草稿箱 · 仅保存在本机" else "Drafts · On this device", Modifier.padding(horizontal = 30.dp)) }
                    items(drafts.filter { it.phase != com.lycoris.maps.feature.contributions.DraftPhase.COMPLETE }, key = { "draft:${it.id}" }, contentType = { "draft" }) { draft ->
                        TextButton({ model.openDraft(draft.id) }, Modifier.fillMaxWidth().padding(horizontal = 30.dp)) {
                            Column(Modifier.fillMaxWidth()) {
                                Text(draft.fields.title.ifBlank { if (zh) "未完成的草稿" else "Unfinished draft" })
                                Text(java.text.DateFormat.getDateTimeInstance(java.text.DateFormat.SHORT,
                                    java.text.DateFormat.SHORT, if (zh) java.util.Locale.SIMPLIFIED_CHINESE else java.util.Locale.ENGLISH)
                                    .format(java.util.Date(draft.updatedAt)), style = MaterialTheme.typography.labelSmall)
                            }
                        }
                    }
                    placeItems(PlaceListState(account.createdPlaces, preferences.language, account.createdLoading, account.failure, account.initialized), model.container.clients, account, zh, model::detail, model::myPlaces)
                }
                SecondaryPage.CONTRIBUTION -> item(key = "contribution-content", contentType = "form") { ContributionPanel(drafts.firstOrNull { it.id == draftId }, preferences.language, model::draftCommand, model.container.contributions) }
                null -> when (section) {
                    MainSection.EXPLORE -> {
                        item(key = "positions-title", contentType = "heading") { PanelTitle(if (zh) "点位" else "Positions", chinese = zh, startPadding = 11.dp, endPadding = 11.dp, bottomPadding = 11.dp) }
                        placeItems(viewport, model.container.clients, account, zh, model::detail, model::retryViewport, horizontalPadding = 11.dp)
                    }
                    MainSection.BOOKMARKS -> if (account.user == null) item(key = "bookmark-sign-in", contentType = "account-prompt") {
                        Column(Modifier.padding(horizontal = 11.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                            Text(if (zh) "登录后收藏点位\n或参与贡献。" else "Sign in to bookmark places\nor contribute.")
                            Button(model::account, Modifier.fillMaxWidth()) { Text(if (zh) "登录" else "Log in") }
                        }
                    } else placeItems(PlaceListState(account.favoritePlaces, preferences.language, account.favoritesLoading, account.failure, account.favoritesInitialized), model.container.clients, account, zh, model::detail, { model.selectSection(MainSection.BOOKMARKS) }, horizontalPadding = 11.dp)
                    MainSection.SETTINGS -> Unit
                }
            }
        })
    }
    setting?.let { SettingsDialog(it, preferences, { setting = null }, { value -> map.camera = map.snapshotCamera(); model.language(value) }, model::radius,
        onSearchType = model::searchType,
        onMapSource = { source -> map.camera = map.snapshotCamera(); model.mapSource(source) },
        googleAvailability = model.container.googleMapsAvailability) }
    if (needsTencentPrivacy && setting == null) TencentPrivacyDialog(preferences.language,
        onAccept = model::acceptTencentPrivacy,
        onUseOsm = { model.mapSource(MapSource.OSM) },
        onOpenPrivacy = {
            runCatching { context.startActivity(android.content.Intent(android.content.Intent.ACTION_VIEW,
                android.net.Uri.parse("https://lbs.qq.com/userAgreements/agreements/privacy"))) }
                .onFailure { model.message(if (zh) "没有可用的浏览器。" else "No browser is available.") }
        })
    webFallback?.let { destination ->
        AlertDialog(onDismissRequest = { webFallback = null }, title = { Text(if (zh) "没有可用的导航应用" else "No navigation app available") },
            text = { Text(if (zh) "可以在浏览器中查看目的地。" else "You can view the destination in a browser.") },
            confirmButton = { TextButton({
                webFallback = null
                if (navigation.openWebFallback(context, destination, if (zh) "打开地图网站" else "Open map website") == ExternalActionResult.Unavailable) model.message(if (zh) "没有可用的浏览器。" else "No browser available.")
            }) { Text(if (zh) "打开地图网站" else "Open map website") } }, dismissButton = { TextButton({ webFallback = null }) { Text(if (zh) "取消" else "Cancel") } })
    }
    if (devices.voice.isActive) AlertDialog(onDismissRequest = devices.cancelVoice,
        title = { Text(if (zh) "语音搜索" else "Voice search") }, text = { Text(devices.voice.transcript.ifBlank { if (zh) "请说出要搜索的点位…" else "Say the place you’re looking for…" }) },
        confirmButton = { TextButton(devices.cancelVoice) { Text(if (zh) "取消" else "Cancel") } })
}
