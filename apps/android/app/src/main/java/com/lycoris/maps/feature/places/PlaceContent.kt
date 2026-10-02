package com.lycoris.maps.feature.places

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Directions
import androidx.compose.material.icons.rounded.Bookmark
import androidx.compose.material.icons.rounded.BookmarkBorder
import androidx.compose.material.icons.rounded.Edit
import androidx.compose.material.icons.rounded.Share
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.Dp
import coil3.ImageLoader
import coil3.compose.SubcomposeAsyncImage
import coil3.network.okhttp.OkHttpNetworkFetcherFactory
import coil3.request.CachePolicy
import coil3.request.ImageRequest
import coil3.request.crossfade
import com.lycoris.maps.core.data.AccountState
import com.lycoris.maps.core.data.PlaceListState
import com.lycoris.maps.core.designsystem.LycorisTextStyles
import com.lycoris.maps.core.designsystem.LycorisColors
import com.lycoris.maps.core.model.Language
import com.lycoris.maps.core.model.Marker
import com.lycoris.maps.core.network.ApiClients
import com.lycoris.maps.core.network.ImageVariant
import com.lycoris.maps.feature.map.displayMessage
import kotlin.math.*

fun LazyListScope.placeItems(state: PlaceListState, clients: ApiClients, account: AccountState, chinese: Boolean, onPlace: (Long) -> Unit, onRetry: () -> Unit, horizontalPadding: Dp = 30.dp) {
        if (state.loading) item(key = "places-loading", contentType = "status") {
            LinearProgressIndicator(Modifier.fillMaxWidth().padding(horizontal = horizontalPadding))
        }
        if (state.failure != null) item(key = "places-failure", contentType = "status") {
            Column(Modifier.fillMaxWidth().padding(horizontal = horizontalPadding)) {
            Text(state.failure.displayMessage(if (chinese) Language.ZH else Language.EN), style = MaterialTheme.typography.bodyMedium)
            TextButton(onRetry) { Text(if (chinese) "重试" else "Retry") }
            }
        }
        if (state.loaded && state.places.isEmpty() && !state.loading && state.failure == null) item(key = "places-empty", contentType = "status") {
            Text(if (chinese) "这个范围内暂时没有点位。" else "No places found in this area.", Modifier.padding(horizontal = horizontalPadding), color = LycorisColors.SecondaryText)
        }
        itemsIndexed(state.places, key = { _, place -> "place:${place.id}" }, contentType = { _, _ -> "place" }) { index, place ->
            PlaceRow(place, clients, account, chinese, onPlace, index < state.places.lastIndex, horizontalPadding)
        }
}

@Composable
private fun PlaceRow(place: Marker, clients: ApiClients, account: AccountState, chinese: Boolean, onPlace: (Long) -> Unit, hasFollowingRow: Boolean, horizontalPadding: Dp) {
        Box(Modifier.fillMaxWidth().padding(horizontal = horizontalPadding).padding(bottom = if (hasFollowingRow) 16.dp else 0.dp)) {
            Surface(Modifier.fillMaxWidth().clickable(role = Role.Button, onClick = { onPlace(place.id) }), shape = RoundedCornerShape(24.dp), color = LycorisColors.Card) {
                Column {
                    if (!place.markImage.isNullOrBlank()) PlacePhoto(place, clients, account, Modifier.fillMaxWidth().aspectRatio(16f / 9).clip(RoundedCornerShape(16.dp)), false, chinese)
                    Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text(place.title, style = LycorisTextStyles.PlaceTitle)
                        Text(categoryName(place.category, chinese), style = MaterialTheme.typography.labelMedium, color = LycorisColors.SecondaryText)
                        PlaceVenueTag(place, chinese)
                        PlaceHours(place, chinese)
                        place.description?.takeIf { it.isNotBlank() }?.let { Text(it, style = LycorisTextStyles.PlaceSummary, color = LycorisColors.SecondaryText, maxLines = 3, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis) }
                    }
                }
            }
        }
}

@Composable
fun PlaceDetailContent(place: Marker, clients: ApiClients, account: AccountState, chinese: Boolean, referenceLat: Double, referenceLng: Double, onFavorite: () -> Unit, onNavigate: () -> Unit, onShare: () -> Unit, onEdit: () -> Unit) {
    Column(Modifier.fillMaxWidth().padding(horizontal = 30.dp), verticalArrangement = Arrangement.spacedBy(11.dp)) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(11.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(categoryName(place.category, chinese), Modifier.weight(1f), color = LycorisColors.SecondaryText)
            Text(distanceLabel(referenceLat, referenceLng, place.lat, place.lng), style = LycorisTextStyles.PlaceDistance, color = LycorisColors.SecondaryText)
        }
        PlaceMetadataRow(place, chinese)
        if (!place.openingHoursNote.isNullOrBlank()) Text(place.openingHoursNote, style = MaterialTheme.typography.bodyMedium)
        var selectedPhoto by remember(place.id) { mutableIntStateOf(0) }
        val album = place.photos.map { it.url }.ifEmpty { listOfNotNull(place.markImage) }
        val shown = if (album.isEmpty()) place else place.copy(markImage = album[selectedPhoto.coerceIn(album.indices)])
        if (!shown.markImage.isNullOrBlank()) PlacePhoto(shown, clients, account, Modifier.fillMaxWidth().aspectRatio(16f / 9).clip(RoundedCornerShape(16.dp)), true, chinese)
        if (album.size > 1) Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
            TextButton({ selectedPhoto-- }, enabled = selectedPhoto > 0) { Text(if (chinese) "上一张" else "Previous photo") }
            Text("${selectedPhoto + 1} / ${album.size}")
            TextButton({ selectedPhoto++ }, enabled = selectedPhoto < album.lastIndex) { Text(if (chinese) "下一张" else "Next photo") }
        }
        place.description?.takeIf(String::isNotBlank)?.let { Text(it, style = LycorisTextStyles.PlaceDescription) }
        if (place.reviewStatus != "APPROVED") Text(if (chinese) "待审核" else "Pending review", color = LycorisColors.Primary)
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End, verticalAlignment = Alignment.CenterVertically) {
            IconButton(onShare) { Icon(Icons.Rounded.Share, if (chinese) "分享点位" else "Share place") }
            IconButton(onEdit) { Icon(Icons.Rounded.Edit, if (chinese) "编辑点位" else "Edit place", tint = LycorisColors.Plum) }
            Spacer(Modifier.weight(1f))
            FilledTonalButton(onNavigate) {
                Text(if (chinese) "导航" else "Navigate")
                Spacer(Modifier.width(8.dp))
                Icon(Icons.Rounded.Directions, null, Modifier.size(20.dp))
            }
            IconButton(onFavorite, enabled = place.id !in account.pendingFavoriteIds && (account.user == null || account.favoritesInitialized)) {
                Icon(if (place.id in account.favoriteIds) Icons.Rounded.Bookmark else Icons.Rounded.BookmarkBorder,
                    if (chinese) { if (place.id in account.favoriteIds) "取消收藏" else "收藏点位" } else { if (place.id in account.favoriteIds) "Remove bookmark" else "Bookmark place" }, tint = LycorisColors.Plum)
            }
        }
    }
}

private data class PlaceImages(val public: ImageLoader, val private: ImageLoader)
private val LocalPlaceImages = staticCompositionLocalOf<PlaceImages> { error("Place images require PlaceImageScope") }

@Composable
fun PlaceImageScope(clients: ApiClients, account: AccountState, content: @Composable () -> Unit) {
    val context = LocalContext.current.applicationContext
    val publicLoader = remember(clients) {
        ImageLoader.Builder(context).components { add(OkHttpNetworkFetcherFactory(callFactory = { clients.publicMediaClient })) }.build()
    }
    val privateLoader = remember(clients, account.epoch) {
        ImageLoader.Builder(context).diskCache(null).components {
            add(OkHttpNetworkFetcherFactory(callFactory = { clients.authenticatedClient(account.epoch) }))
        }.build()
    }
    DisposableEffect(publicLoader) { onDispose { publicLoader.shutdown() } }
    DisposableEffect(privateLoader) { onDispose { privateLoader.memoryCache?.clear(); privateLoader.shutdown() } }
    PlaceTimeScope { CompositionLocalProvider(LocalPlaceImages provides PlaceImages(publicLoader, privateLoader), content = content) }
}

@Composable
fun PlacePhoto(place: Marker, clients: ApiClients, account: AccountState, modifier: Modifier, detail: Boolean, chinese: Boolean) {
    val context = LocalContext.current
    val resource = clients.resolveImage(place.markImage, if (detail) ImageVariant.DETAIL else ImageVariant.THUMB, place.publiclyVisible) ?: return
    val private = !place.publiclyVisible && account.user != null
    val images = LocalPlaceImages.current
    val loader = if (private) images.private else images.public
    val cacheKey = "${clients.origin}|${if (private) "${account.user?.publicId}|${account.epoch}" else "public"}|${resource.url}"
    SubcomposeAsyncImage(
        model = ImageRequest.Builder(context).data(resource.url.toString()).crossfade(true)
            .memoryCacheKey(cacheKey).diskCacheKey(cacheKey)
            .diskCachePolicy(if (private) CachePolicy.DISABLED else CachePolicy.ENABLED).build(),
        imageLoader = loader,
        contentDescription = if (chinese) "${place.title}的图片" else "Photo of ${place.title}",
        modifier = modifier.background(LycorisColors.Card), contentScale = ContentScale.Crop,
        loading = { Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp) } },
        error = { Box(Modifier.fillMaxSize().padding(16.dp), contentAlignment = Alignment.Center) { Text(if (chinese) "图片暂时无法加载" else "Photo unavailable", color = LycorisColors.SecondaryText, style = MaterialTheme.typography.bodySmall) } },
    )
}

fun categoryName(key: String, chinese: Boolean): String = when (key) {
    "accessible_toilet" -> if (chinese) "无障碍卫生间" else "Accessible toilet"
    "baby_room" -> if (chinese) "母婴室" else "Nursing room"
    "friendly_clinic" -> if (chinese) "医疗机构" else "Medical institution"
    else -> if (chinese) "其他" else "Other"
}
fun distanceLabel(lat: Double, lng: Double, otherLat: Double, otherLng: Double): String {
    val radians = Math.PI / 180
    val a = sin((otherLat - lat) * radians / 2).pow(2) + cos(lat * radians) * cos(otherLat * radians) * sin((otherLng - lng) * radians / 2).pow(2)
    val meters = 6371008.8 * 2 * asin(sqrt(a.coerceIn(0.0, 1.0)))
    return if (meters < 1000) "${meters.roundToInt()}m" else String.format(java.util.Locale.ROOT, "%.1fkm", meters / 1000)
}
