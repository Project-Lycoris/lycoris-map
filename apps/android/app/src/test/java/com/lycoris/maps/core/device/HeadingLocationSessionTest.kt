package com.lycoris.maps.core.device

import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class HeadingLocationSessionTest {
    private val firstTime = 100_000_000_000L
    private fun fix(time: Long = firstTime, approximate: Boolean = false) =
        DeviceLocation(31.2, 121.4, if (approximate) 2500f else 15f, null,
            1_750_000_000_000L, time, approximate)

    private fun located(fix: DeviceLocation = fix(), status: LocationStatus = LocationStatus.READY) =
        LocationState(status, if (fix.approximate) LocationPermission.APPROXIMATE else LocationPermission.PRECISE, fix)

    private class Sensor {
        var running = false
        val events = mutableListOf<String>()
        var location: DeviceLocation? = null
        fun update(fix: DeviceLocation?) { location = fix; events += "location:${fix?.elapsedRealtimeNanos}" }
        fun start() { assertNotNull("Set the location before starting direction", location); running = true; events += "start" }
        fun stop() { running = false; events += "stop" }
    }

    private fun TestScope.observe(locations: MutableStateFlow<LocationState>, sensor: Sensor, now: () -> Long = { firstTime }) =
        backgroundScope.launch { collectLocatedHeading(locations, now, sensor::update, sensor::start, sensor::stop) }

    @Test fun `permission and first approximate fix start direction in the same session`() = runTest {
        val locations = MutableStateFlow(LocationState(LocationStatus.PERMISSION_REQUIRED))
        val sensor = Sensor()
        val session = observe(locations, sensor)
        runCurrent()
        assertFalse(sensor.running)
        locations.value = LocationState(LocationStatus.ACQUIRING, LocationPermission.APPROXIMATE)
        runCurrent()
        assertFalse("Permission alone is not a location", sensor.running)
        val coarse = fix(approximate = true)
        locations.value = located(coarse)
        runCurrent()
        assertTrue(sensor.running)
        assertEquals(coarse, sensor.location)
        assertEquals(1, sensor.events.count { it == "start" })
        session.cancelAndJoin()
        assertFalse(sensor.running)
    }

    @Test fun `fresh location updates and reacquisition do not restart direction smoothing`() = runTest {
        var now = firstTime
        val locations = MutableStateFlow(located())
        val sensor = Sensor()
        val session = observe(locations, sensor) { now }
        runCurrent()
        locations.value = located(status = LocationStatus.ACQUIRING)
        runCurrent()
        assertTrue("A fresh retained fix remains usable while acquiring", sensor.running)
        now += 2_000_000_000L
        locations.value = located(fix(now))
        runCurrent()
        assertEquals(now, sensor.location?.elapsedRealtimeNanos)
        assertEquals(1, sensor.events.count { it == "start" })
        assertEquals(0, sensor.events.count { it == "stop" })
        session.cancelAndJoin()
        assertFalse(sensor.running)
    }

    @Test fun `permission downgrade and revocation stop until a permitted replacement fix arrives`() = runTest {
        val locations = MutableStateFlow(located())
        val sensor = Sensor()
        val session = observe(locations, sensor)
        runCurrent()
        assertTrue(sensor.running)
        // refreshPermission clears the precise fix before beginning approximate acquisition.
        locations.value = LocationState(permission = LocationPermission.APPROXIMATE)
        runCurrent()
        assertFalse(sensor.running)
        assertNull(sensor.location)
        locations.value = located(fix(approximate = true))
        runCurrent()
        assertTrue(sensor.running)
        locations.value = LocationState(LocationStatus.PERMISSION_REQUIRED)
        runCurrent()
        assertFalse(sensor.running)
        assertNull(sensor.location)
        assertEquals(2, sensor.events.count { it == "start" })
        session.cancelAndJoin()
    }

    @Test fun `fix expiry and disabled providers stop direction and a later fix restarts it`() = runTest {
        var now = firstTime
        val locations = MutableStateFlow(located())
        val sensor = Sensor()
        val session = observe(locations, sensor) { now }
        runCurrent()
        now += 60_001_000_000L
        // LocationController.watchFixExpiry emits ACQUIRING without the expired fix.
        locations.value = LocationState(LocationStatus.ACQUIRING, LocationPermission.PRECISE)
        runCurrent()
        assertFalse(sensor.running)
        locations.value = located(fix(now))
        runCurrent()
        assertTrue(sensor.running)
        locations.value = LocationState(LocationStatus.DISABLED, LocationPermission.PRECISE)
        runCurrent()
        assertFalse(sensor.running)
        assertNull(sensor.location)
        session.cancelAndJoin()
    }

    @Test fun `rotation cancels the old session and replacement uses the current fix`() = runTest {
        val locations = MutableStateFlow(located())
        val oldSensor = Sensor()
        val oldSession = observe(locations, oldSensor)
        runCurrent()
        assertTrue(oldSensor.running)
        oldSession.cancelAndJoin()
        assertFalse(oldSensor.running)
        val replacement = Sensor()
        val newSession = observe(locations, replacement)
        runCurrent()
        assertTrue(replacement.running)
        assertEquals(locations.value.fix, replacement.location)
        assertEquals(1, oldSensor.events.count { it == "start" })
        newSession.cancelAndJoin()
        assertFalse(replacement.running)
    }

    @Test fun `background return rejects retained stopped and stale fixes before resuming`() = runTest {
        var now = firstTime
        val locations = MutableStateFlow(located(status = LocationStatus.STOPPED))
        val sensor = Sensor()
        val session = observe(locations, sensor) { now }
        runCurrent()
        assertFalse(sensor.running)
        now += 60_001_000_000L
        locations.value = located(status = LocationStatus.ACQUIRING)
        runCurrent()
        assertFalse("An expired cached fix must not start direction after resume", sensor.running)
        locations.value = located(fix(now))
        runCurrent()
        assertTrue(sensor.running)
        session.cancelAndJoin()
        assertFalse(sensor.running)
    }
}
