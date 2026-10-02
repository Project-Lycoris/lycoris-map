package com.lycoris.maps.feature.places

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.*
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Schedule
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import com.lycoris.maps.core.map.categoryColor
import com.lycoris.maps.core.model.PlaceCategory
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import com.lycoris.maps.core.model.Marker
import com.lycoris.maps.core.model.OpeningStatus
import com.lycoris.maps.core.model.hoursLabel
import com.lycoris.maps.core.model.openingStatus
import com.lycoris.maps.core.model.venue
import java.time.Instant
import java.time.Clock
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive

internal val LocalPlaceTime = staticCompositionLocalOf { Instant.now() }

/** One foreground clock for visible rows/detail, refreshed immediately after returning to the app. */
@Composable
fun PlaceTimeScope(clock: Clock = Clock.systemUTC(), content: @Composable () -> Unit) {
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val now by produceState(clock.instant(), lifecycle, clock) {
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            while (isActive) {
                value = clock.instant()
                delay(60_000L - Math.floorMod(clock.millis(), 60_000L))
            }
        }
    }
    CompositionLocalProvider(LocalPlaceTime provides now, content = content)
}

@Composable
fun PlaceVenueTag(place: Marker, chinese: Boolean) {
    val venue = place.venue ?: return
    MetadataTag(venue.label(chinese))
}

/** Informational Material surfaces: no fake click action or disabled interactive chip. */
@Composable
private fun MetadataTag(label: String, warning: Boolean = false) {
    Surface(shape = MaterialTheme.shapes.small,
        border = if (warning) null else BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
        color = if (warning) MaterialTheme.colorScheme.errorContainer else MaterialTheme.colorScheme.secondaryContainer,
        contentColor = if (warning) MaterialTheme.colorScheme.onErrorContainer else MaterialTheme.colorScheme.onSecondaryContainer) {
        Text(label, Modifier.padding(horizontal = 10.dp, vertical = 6.dp), style = MaterialTheme.typography.labelMedium)
    }
}

@Composable
fun PlaceHours(place: Marker, chinese: Boolean) {
    if (place.hoursLabel(chinese) == null) return
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) { HoursBadges(place, chinese) }
}

/** Keep detail metadata together, wrapping only when text or the available width requires it. */
@Composable
fun PlaceMetadataRow(place: Marker, chinese: Boolean) {
    FlowRow(horizontalArrangement = Arrangement.spacedBy(11.dp), verticalArrangement = Arrangement.spacedBy(11.dp),
        itemVerticalAlignment = Alignment.CenterVertically) {
        if (place.facilityCategories.size > 1) place.facilityCategories.forEach { category ->
            val color = Color(categoryColor(PlaceCategory.fromWire(category)))
            Surface(shape = MaterialTheme.shapes.small, color = color.copy(alpha = 0.16f),
                border = BorderStroke(1.dp, color.copy(alpha = 0.35f))) {
                Text(categoryName(category, chinese), Modifier.padding(horizontal = 10.dp, vertical = 6.dp),
                    style = MaterialTheme.typography.labelMedium)
            }
        }
        PlaceVenueTag(place, chinese)
        HoursBadges(place, chinese)
    }
}

@Composable
private fun HoursBadges(place: Marker, chinese: Boolean) {
    val hours = place.hoursLabel(chinese) ?: return
    val status = place.openingStatus(LocalPlaceTime.current)
    val label = when (status) {
        OpeningStatus.OPEN, OpeningStatus.CLOSING_SOON -> if (chinese) "营业中" else "Open now"
        OpeningStatus.CLOSED -> if (chinese) "已结束营业" else "Closed now"
        else -> null
    }
    val warning = status == OpeningStatus.CLOSING_SOON
    Surface(shape = MaterialTheme.shapes.small,
        color = if (warning) MaterialTheme.colorScheme.errorContainer else MaterialTheme.colorScheme.primaryContainer,
        contentColor = if (warning) MaterialTheme.colorScheme.onErrorContainer else MaterialTheme.colorScheme.onPrimaryContainer) {
        Row(Modifier.padding(horizontal = 10.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Icon(Icons.Rounded.Schedule, null, Modifier.size(18.dp))
            Text(listOfNotNull(label, hours).joinToString(" · "), style = MaterialTheme.typography.labelLarge, fontWeight = FontWeight.Medium)
        }
    }
    if (warning) MetadataTag(if (chinese) "即将结束营业" else "Closing soon", warning = true)
}
