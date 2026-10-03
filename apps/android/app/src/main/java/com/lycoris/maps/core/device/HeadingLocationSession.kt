package com.lycoris.maps.core.device

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.collect

/** Collect on the main thread while the map is STARTED; cancellation releases the sensor. */
internal suspend fun collectLocatedHeading(
    locations: Flow<LocationState>,
    nowNanos: () -> Long,
    updateLocation: (DeviceLocation?) -> Unit,
    startHeading: () -> Unit,
    stopHeading: () -> Unit,
) {
    var active = false
    try {
        locations.collect { location ->
            // Acquisition can retain a fresh previous fix. A stopped controller can also retain
            // one, but that must not restart direction sensing until location acquisition resumes.
            val fix = location.fix?.takeIf {
                location.permission != LocationPermission.NONE &&
                    location.status in setOf(LocationStatus.READY, LocationStatus.ACQUIRING) &&
                    LocationFixPolicy.isValid(it, nowNanos())
            }
            if (fix == null && active) {
                stopHeading()
                active = false
            }
            updateLocation(fix)
            if (fix != null && !active) {
                startHeading()
                active = true
            }
        }
    } finally {
        stopHeading()
    }
}
