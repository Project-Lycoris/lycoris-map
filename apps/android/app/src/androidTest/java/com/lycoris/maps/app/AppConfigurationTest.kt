package com.lycoris.maps.app

import android.content.Context
import android.content.SharedPreferences
import android.content.pm.ActivityInfo
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.graphics.Rect
import android.os.Build
import android.provider.Settings
import android.view.View
import android.view.ViewGroup
import android.view.inputmethod.InputMethodManager
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.ViewModelProvider
import androidx.test.core.app.ActivityScenario
import androidx.test.espresso.Espresso
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lycoris.maps.BuildConfig
import com.lycoris.maps.core.device.QA_LOCAL_NETWORK_REQUESTED
import com.lycoris.maps.core.network.LycorisJson
import com.lycoris.maps.core.network.QA_ENVIRONMENT
import com.lycoris.maps.core.network.QaManifest
import com.lycoris.maps.feature.map.HomeViewModel
import com.lycoris.maps.feature.map.MainSection
import com.lycoris.maps.feature.map.SecondaryPage
import java.util.concurrent.atomic.AtomicReference
import okhttp3.Request
import org.junit.After
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.maplibre.android.camera.CameraPosition
import org.maplibre.android.camera.CameraUpdateFactory
import org.maplibre.android.geometry.LatLng
import org.maplibre.android.maps.MapLibreMap
import org.maplibre.android.maps.MapView

/** Real system IME and configuration changes, with no credential submission or server writes. */
@RunWith(AndroidJUnit4::class)
class AppConfigurationTest {
    @get:Rule val compose = createEmptyComposeRule()
    private val context: Context get() = InstrumentationRegistry.getInstrumentation().targetContext
    private var scenario: ActivityScenario<MainActivity>? = null
    private var permissionPreferences: SharedPreferences? = null
    private var previousLocationRequested: Boolean? = null
    private var previousQaLocalNetworkRequested: Boolean? = null
    private var previousRequestedOrientation: Int? = null
    private var previousConfigurationOrientation: Int? = null
    private var rotationStateDiagnostics: RotationStateDiagnostics? = null

    @Before fun launchRealQaActivity() {
        assertTrue("Configuration tests require the QA build", BuildConfig.TEST_ENVIRONMENT)
        assertEquals("com.lycoris.maps.qa", context.packageName)
        assertEquals("http://10.0.2.2:18187/", BuildConfig.API_BASE_URL)
        permissionPreferences = context.getSharedPreferences("device-permissions", Context.MODE_PRIVATE).also {
            previousLocationRequested = if (it.contains("location-requested")) it.getBoolean("location-requested", false) else null
            previousQaLocalNetworkRequested = if (it.contains(QA_LOCAL_NETWORK_REQUESTED)) it.getBoolean(QA_LOCAL_NETWORK_REQUESTED, false) else null
            assertTrue(it.edit().putBoolean("location-requested", true).putBoolean(QA_LOCAL_NETWORK_REQUESTED, true).commit())
        }
        scenario = ActivityScenario.launch(MainActivity::class.java)
        scenario!!.onActivity {
            previousRequestedOrientation = it.requestedOrientation
            previousConfigurationOrientation = it.resources.configuration.orientation
        }
        compose.onNode(tab("Explore", "探索")).assertIsSelected()
    }

    @After fun restoreActivityPolicyAndPermissionMarker() {
        try {
            // Restore this Activity's orientation, never a global accelerometer/rotation setting.
            previousConfigurationOrientation?.let(::requestConfiguration)
            previousRequestedOrientation?.let { original ->
                scenario?.onActivity { it.requestedOrientation = original }
                InstrumentationRegistry.getInstrumentation().waitForIdleSync()
                scenario?.onActivity { assertEquals("Activity orientation policy was not restored", original, it.requestedOrientation) }
            }
        } finally {
            try {
                scenario?.close()
            } finally {
                rotationStateDiagnostics?.close()
                permissionPreferences?.edit()?.apply {
                    val original = previousLocationRequested
                    if (original == null) remove("location-requested") else putBoolean("location-requested", original)
                    val originalLocalNetwork = previousQaLocalNetworkRequested
                    if (originalLocalNetwork == null) remove(QA_LOCAL_NETWORK_REQUESTED) else putBoolean(QA_LOCAL_NETWORK_REQUESTED, originalLocalNetwork)
                }?.let { assertTrue("Could not restore the QA permission marker", it.commit()) }
            }
        }
    }

    @Test fun realImeKeepsSearchAndLoginOpenOnFirstBackAndLastButtonReachable() {
        requestConfiguration(Configuration.ORIENTATION_PORTRAIT)
        val manager = context.getSystemService(InputMethodManager::class.java)
        val selected = Settings.Secure.getString(context.contentResolver, Settings.Secure.DEFAULT_INPUT_METHOD)
        assertTrue("This device must have a selected, enabled system IME; the test does not change global keyboard settings",
            !selected.isNullOrBlank() && manager.enabledInputMethodList.any { it.id == selected })

        val query = "Android QA IME search"
        val searchField = compose.onNode(hasSetTextAction())
        searchField.assertIsDisplayed().assertIsEnabled().performTouchInput { click() }
        assertTouchFocused(searchField, "Search")
        awaitIme(true, "Touching the search field must open the real system keyboard")
        compose.onNode(hasSetTextAction()).performTextInput(query)
        withModel { assertEquals(query, it.query.value); assertEquals(SecondaryPage.SEARCH, it.page.value) }
        Espresso.pressBack() // Real system key dispatch: the IME must consume this before app navigation.
        awaitIme(false, "The first system Back must hide the search keyboard")
        withModel { assertEquals(query, it.query.value); assertEquals(SecondaryPage.SEARCH, it.page.value) }
        scenario!!.onActivity { assertFalse("IME dismissal must not finish MainActivity", it.isFinishing) }
        Espresso.pressBack()
        compose.onNode(tab("Explore", "探索")).assertIsSelected()
        withModel { assertNull(it.page.value) }

        val account = (context.applicationContext as LycorisApplication).container.accounts
        compose.waitUntil(10_000) { account.state.value.initialized && !account.state.value.busy }
        assertNull("This login case requires a signed-out QA package; it never clears an existing session", account.state.value.user)
        compose.onNode(description("Account", "账号")).performClick()
        compose.onNode(SemanticsMatcher.keyIsDefined(SemanticsProperties.PaneTitle))
            .performSemanticsAction(SemanticsActions.Expand) { it() }
        val username = compose.onNode(hasSetTextAction() and text("Username or Email", "账号或邮箱"))
        username.performScrollTo().performTouchInput { click() }
        assertTouchFocused(username, "Login")
        awaitIme(true, "Touching the login field must open the real system keyboard")
        username.performTextInput("synthetic-ime-only")
        compose.onNode(text("Log In", "登录") and hasClickAction()).assertIsNotEnabled()
        val finalButton = compose.onNode(text("Create an Account", "创建账号") and hasClickAction())
        finalButton.performScrollTo().assertIsDisplayed().assertIsEnabled()
        assertAboveIme(finalButton)
        Espresso.pressBack()
        awaitIme(false, "The first system Back must hide the login keyboard")
        withModel { assertEquals(SecondaryPage.ACCOUNT, it.page.value) }
        username.assertTextContains("synthetic-ime-only")
        compose.onNode(text("Log In", "登录") and hasClickAction()).assertIsNotEnabled()
    }

    @Test fun coldStartKeepsImeHiddenAndShowsPrimaryNavigation() {
        // No touch happens here: the map must open without the IME, and the primary navigation
        // must actually be laid out and drawn rather than hidden behind an unintended keyboard.
        val manager = context.getSystemService(InputMethodManager::class.java)
        val selected = Settings.Secure.getString(context.contentResolver, Settings.Secure.DEFAULT_INPUT_METHOD)
        assertTrue("This device must have a selected, enabled system IME; the test does not change global keyboard settings",
            !selected.isNullOrBlank() && manager.enabledInputMethodList.any { it.id == selected })

        var focused = false
        compose.waitUntil(10_000) {
            scenario!!.onActivity { focused = it.window.decorView.hasWindowFocus() }
            focused
        }
        // Let the platform process frames after focus arrives; Compose's test clock alone does
        // not drive the real input method. Choreographer callbacks are not proof of GPU rendering.
        awaitPlatformFrames(6)
        awaitIme(false, "An untouched cold start must not show the system keyboard")
        // Re-check after additional real frames: no touch occurred, so the IME must stay hidden.
        awaitPlatformFrames(6)
        awaitIme(false, "The keyboard must remain hidden while the user has not touched any field")

        compose.onNode(tab("Explore", "探索")).assertIsDisplayed().assertIsSelected()
        compose.onNode(tab("Bookmarks", "收藏")).assertIsDisplayed()
        compose.onNode(tab("Settings", "设置")).assertIsDisplayed()
        withModel { assertNull(it.page.value) }
    }

    /** Wait for platform frames without changing a system keyboard or animation setting. */
    private fun awaitPlatformFrames(count: Int) {
        val latch = java.util.concurrent.CountDownLatch(count)
        lateinit var choreographer: android.view.Choreographer
        lateinit var callback: android.view.Choreographer.FrameCallback
        scenario!!.onActivity {
            choreographer = android.view.Choreographer.getInstance()
            callback = object : android.view.Choreographer.FrameCallback {
                override fun doFrame(frameTimeNanos: Long) {
                    latch.countDown()
                    if (latch.count > 0) choreographer.postFrameCallback(this)
                }
            }
            choreographer.postFrameCallback(callback)
        }
        try {
            assertTrue("The window did not receive $count platform frames", latch.await(10, java.util.concurrent.TimeUnit.SECONDS))
        } finally {
            InstrumentationRegistry.getInstrumentation().runOnMainSync { choreographer.removeFrameCallback(callback) }
        }
    }

    @Test fun actualOrientationRebuildRetainsPrimarySelectionCameraAndSelectedPlaceId() {
        rotationStateDiagnostics = RotationStateDiagnostics()
        requestConfiguration(Configuration.ORIENTATION_PORTRAIT)
        compose.onNode(tab("Settings", "设置")).performClick()
        // Drag the actual handle, then observe unclipped panel geometry rather than private state.
        compose.onNodeWithTag("panel-grabber").performTouchInput {
            swipe(center, center.copy(y = center.y - 400f), 450)
        }
        assertFullyExpandedPanel()
        val expandedTop = compose.onNodeWithTag("panel-grabber").fetchSemanticsNode().boundsInWindow.top
        withModel { it.mapGesture() }
        val portrait = nativeMap()
        val camera = userCamera(31.241, 121.492)
        moveCamera(portrait, camera)

        requestConfiguration(Configuration.ORIENTATION_LANDSCAPE)
        compose.onNode(tab("Settings", "设置")).assertIsSelected()
        withModel { assertEquals(MainSection.SETTINGS, it.section.value); assertNull(it.page.value) }
        val landscape = nativeMap()
        assertNotSame("Rotation must create a new Activity", portrait.activity, landscape.activity)
        assertNotSame("The recreated Activity must own a new native MapView", portrait.view, landscape.view)
        assertCamera(landscape, camera)
        assertFullyExpandedPanel()

        // A short landscape viewport can merge middle/full. Returning to portrait must retain
        // the user's expanded intent, not reset to the otherwise 22 dp shorter middle state.
        requestConfiguration(Configuration.ORIENTATION_PORTRAIT)
        val secondPortrait = nativeMap()
        assertNotSame(landscape.activity, secondPortrait.activity)
        assertFullyExpandedPanel()
        assertEquals("Expanded Settings must return to its previous visible height", expandedTop,
            compose.onNodeWithTag("panel-grabber").fetchSemanticsNode().boundsInWindow.top, 2f)
        assertCamera(secondPortrait, camera)

        // This ordinary CI case asserts the chosen ID even when the backend is absent.
        // The opt-in case below separately requires a loaded place before checking recentering.
        withModel { it.detail(2L) }
        compose.waitForIdle()
        requestConfiguration(Configuration.ORIENTATION_LANDSCAPE)
        withModel { assertEquals(SecondaryPage.DETAIL, it.page.value); assertEquals(2L, it.places.detail.value.id) }
        val restored = nativeMap()
        assertNotSame(secondPortrait.activity, restored.activity)
        compose.onNode(description("Close", "关闭")).performClick()
        compose.onNode(tab("Settings", "设置")).assertIsSelected()
        compose.onNode(tab("Explore", "探索")).performClick().assertIsSelected()
        compose.onNode(text("Find Nearby", "查找附近")).assertIsDisplayed()
    }

    @Test fun loadedQaPlaceKeepsUserCameraThroughRealOrientationRoundTrip() {
        assumeTrue("Loaded-place rotation is read-only opt-in: lycorisQaLoadedDetailRotation=true",
            InstrumentationRegistry.getArguments().getString("lycorisQaLoadedDetailRotation") == "true")
        if (Build.VERSION.SDK_INT >= 37) {
            assertEquals("The selected QA user needs the scoped local-network test permission; this test does not change it",
                PackageManager.PERMISSION_GRANTED, context.checkSelfPermission("android.permission.ACCESS_LOCAL_NETWORK"))
        }
        val clients = (context.applicationContext as LycorisApplication).container.clients
        val manifest = clients.publicClient.newCall(Request.Builder()
            .url("http://10.0.2.2:18187/__lycoris_qa__/manifest").get().build()).execute().use { response ->
            assertEquals("The fixed isolated QA gateway must be ready", 200, response.code)
            assertEquals(QA_ENVIRONMENT, response.header("X-Lycoris-Test-Environment"))
            val source = response.body.source()
            source.request(16_385)
            assertTrue("QA manifest is too large", source.buffer.size <= 16_384)
            LycorisJson.decodeFromString<QaManifest>(source.readUtf8()).also { it.verifiedNonce() }
        }
        requestConfiguration(Configuration.ORIENTATION_PORTRAIT)
        withModel { it.detail(manifest.sentinel.markerId) }
        compose.waitUntil(10_000) {
            var finished = false
            withModel { finished = !it.places.detail.value.loading }
            finished
        }
        var point: LatLng? = null
        withModel {
            val detail = it.places.detail.value
            assertNull("The real isolated QA detail request failed", detail.failure)
            assertNotNull("A selected ID alone does not cover loaded-place rotation", detail.place)
            val place = detail.place!!
            assertEquals(manifest.sentinel.markerId, place.id)
            assertEquals(manifest.sentinel.title, place.title)
            assertEquals(manifest.sentinel.description, place.description)
            assertEquals(manifest.sentinel.ownerPublicId, place.userPublicId)
            assertEquals(manifest.sentinel.clientRequestId, place.clientRequestId)
            assertTrue(place.publiclyVisible)
            point = LatLng(place.lat, place.lng)
            it.mapGesture()
        }
        val initial = nativeMap()
        compose.waitUntil(5_000) {
            var centered = false
            scenario!!.onActivity {
                val position = initial.map.cameraPosition
                centered = position.target?.let { target ->
                    kotlin.math.abs(target.latitude - point!!.latitude) < 0.00001 &&
                        kotlin.math.abs(target.longitude - point!!.longitude) < 0.00001
                } == true && position.zoom >= 14.99
            }
            centered
        }
        val moved = userCamera(point!!.latitude + 0.015, point!!.longitude + 0.021)
        moveCamera(initial, moved)
        var previous = initial
        for (orientation in listOf(Configuration.ORIENTATION_LANDSCAPE, Configuration.ORIENTATION_PORTRAIT)) {
            requestConfiguration(orientation)
            val current = nativeMap()
            assertNotSame(previous.activity, current.activity)
            withModel {
                assertEquals(SecondaryPage.DETAIL, it.page.value)
                assertEquals(manifest.sentinel.markerId, it.places.detail.value.place?.id)
            }
            assertCamera(current, moved)
            previous = current
        }
    }

    private fun requestConfiguration(orientation: Int) {
        require(orientation in setOf(Configuration.ORIENTATION_PORTRAIT, Configuration.ORIENTATION_LANDSCAPE))
        var before: MainActivity? = null
        var requiresRebuild = false
        scenario!!.onActivity {
            before = it
            requiresRebuild = it.resources.configuration.orientation != orientation
            it.requestedOrientation = if (orientation == Configuration.ORIENTATION_PORTRAIT)
                ActivityInfo.SCREEN_ORIENTATION_PORTRAIT else ActivityInfo.SCREEN_ORIENTATION_LANDSCAPE
        }
        compose.waitUntil(10_000) {
            var changed = false
            scenario!!.onActivity {
                changed = it.resources.configuration.orientation == orientation &&
                    it.lifecycle.currentState == Lifecycle.State.RESUMED && (!requiresRebuild || it !== before)
            }
            changed
        }
        scenario!!.onActivity {
            assertEquals("The device must honor this Activity's orientation request", orientation, it.resources.configuration.orientation)
            if (requiresRebuild) assertNotSame("A real configuration change must rebuild MainActivity", before, it)
        }
        val mainClockBefore = runCatching { compose.mainClock.currentTime }.getOrNull()
        val uptimeBefore = android.os.SystemClock.uptimeMillis()
        val recomposerBefore = recomposerDiagnostics()
        val observedStateBefore = rotationStateDiagnostics?.summary()
        try {
            compose.waitForIdle()
        } catch (failure: Throwable) {
            // Attach fixed-key window/inset facts without calling Compose test APIs from the UI
            // thread, and without masking the original idle timeout.
            val uptimeAfter = android.os.SystemClock.uptimeMillis()
            val mainClockAfter = runCatching { compose.mainClock.currentTime }.getOrNull()
            throw AssertionError(
                "Post-rotation waitForIdle failed. ${orientationWindowDiagnostics()} " +
                    "mainClockBefore=$mainClockBefore mainClockAfter=$mainClockAfter " +
                    "uptimeDeltaMs=${uptimeAfter - uptimeBefore} " +
                    "recomposerBefore=$recomposerBefore recomposerAfter=${recomposerDiagnostics()} " +
                    "observedStateBefore=[$observedStateBefore] observedStateAfter=[${rotationStateDiagnostics?.summary()}] " +
                    "fineLocationGranted=${context.checkSelfPermission(android.Manifest.permission.ACCESS_FINE_LOCATION) == PackageManager.PERMISSION_GRANTED} " +
                    "coarseLocationGranted=${context.checkSelfPermission(android.Manifest.permission.ACCESS_COARSE_LOCATION) == PackageManager.PERMISSION_GRANTED}",
                failure,
            )
        }
    }

    /** Reads only window/inset state on the Activity; never touches Compose semantics. */
    private fun orientationWindowDiagnostics(): String {
        var report = "window diagnostics unavailable"
        runCatching {
            scenario!!.onActivity { activity ->
                val decor = activity.window.decorView
                val insets = ViewCompat.getRootWindowInsets(decor)
                fun bottom(type: Int) = insets?.getInsets(type)?.bottom ?: -1
                fun top(type: Int) = insets?.getInsets(type)?.top ?: -1
                report = "decor=${decor.width}x${decor.height}, windowFocus=${decor.hasWindowFocus()}, " +
                    "orientation=${activity.resources.configuration.orientation}, " +
                    "statusTop=${top(WindowInsetsCompat.Type.statusBars())}, statusVisible=${insets?.isVisible(WindowInsetsCompat.Type.statusBars())}, " +
                    "navBottom=${bottom(WindowInsetsCompat.Type.navigationBars())}, navVisible=${insets?.isVisible(WindowInsetsCompat.Type.navigationBars())}, " +
                    "imeBottom=${bottom(WindowInsetsCompat.Type.ime())}, imeVisible=${insets?.isVisible(WindowInsetsCompat.Type.ime())} " +
                    viewRootDiagnostics(decor)
            }
        }
        return report
    }

    /** Best-effort view-tree facts; no forced measure/layout and no Compose semantic APIs. */
    private fun viewRootDiagnostics(decor: View): String {
        val roots = mutableListOf<String>()
        var views = 0
        var attached = 0
        var layoutRequested = 0
        val pending = java.util.ArrayDeque<View>()
        pending.add(decor)
        while (pending.isNotEmpty()) {
            val view = pending.removeFirst()
            views++
            if (view.isAttachedToWindow) attached++
            if (view.isLayoutRequested) layoutRequested++
            if (view is androidx.compose.ui.platform.ViewRootForTest) {
                roots += "root(hasPendingMeasureOrLayout=${view.hasPendingMeasureOrLayout}," +
                    "layoutRequested=${view.view.isLayoutRequested},attached=${view.view.isAttachedToWindow}," +
                    "visibility=${view.view.visibility},size=${view.view.width}x${view.view.height}," +
                    "lifecycleResumed=${view.isLifecycleInResumedState})"
            }
            if (view is ViewGroup) for (index in 0 until view.childCount) pending.add(view.getChildAt(index))
        }
        val rootsReport = "viewRoots=${roots.size}" + if (roots.isEmpty()) "" else " " + roots.joinToString("|")
        return "views=$views attachedViews=$attached layoutRequestedViews=$layoutRequested $rootsReport"
    }

    /** Public Recomposer facts only; state is a Flow and is not collected here. */
    @OptIn(androidx.compose.runtime.InternalComposeApi::class)
    private fun recomposerDiagnostics(): String {
        var report = "recomposer diagnostics unavailable"
        runCatching {
            report = androidx.compose.runtime.Recomposer.runningRecomposers.value.joinToString("|") { recomposer ->
                "recomposer(hasPendingWork=${recomposer.hasPendingWork},changeCount=${recomposer.changeCount})"
            }.ifEmpty { "recomposerCount=0" }
        }
        return report
    }

    private data class NativeMap(val activity: MainActivity, val view: MapView, val map: MapLibreMap)
    private fun nativeMap(): NativeMap {
        val found = AtomicReference<Pair<MainActivity, MapView>?>()
        compose.waitUntil(10_000) {
            scenario!!.onActivity { activity -> findMap(activity.window.decorView)?.let { found.set(activity to it) } }
            found.get() != null
        }
        val (activity, view) = found.get()!!
        val native = AtomicReference<MapLibreMap?>()
        scenario!!.onActivity { assertSame(activity, it); view.getMapAsync { map -> native.set(map) } }
        compose.waitUntil(10_000) {
            var ready = false
            scenario!!.onActivity { ready = native.get()?.style?.isFullyLoaded == true }
            ready
        }
        compose.waitForIdle()
        return NativeMap(activity, view, native.get()!!)
    }
    private fun findMap(view: View): MapView? {
        if (view is MapView) return view
        if (view is ViewGroup) for (index in 0 until view.childCount) findMap(view.getChildAt(index))?.let { return it }
        return null
    }
    private fun userCamera(latitude: Double, longitude: Double) = CameraPosition.Builder()
        .target(LatLng(latitude, longitude)).zoom(16.25).bearing(53.0).tilt(12.0).build()
    private fun moveCamera(handle: NativeMap, camera: CameraPosition) {
        scenario!!.onActivity {
            assertSame(handle.activity, it)
            handle.map.cancelTransitions()
            handle.map.moveCamera(CameraUpdateFactory.newCameraPosition(camera))
        }
        compose.waitForIdle()
        assertCamera(handle, camera)
    }
    private fun assertCamera(handle: NativeMap, expected: CameraPosition) {
        scenario!!.onActivity {
            assertSame(handle.activity, it)
            val actual = handle.map.cameraPosition
            assertEquals(expected.target!!.latitude, actual.target!!.latitude, 0.00001)
            assertEquals(expected.target!!.longitude, actual.target!!.longitude, 0.00001)
            assertEquals(expected.zoom, actual.zoom, 0.00001)
            assertEquals(expected.bearing, actual.bearing, 0.00001)
            assertEquals(expected.tilt, actual.tilt, 0.00001)
        }
    }

    private fun awaitIme(visible: Boolean, explanation: String) {
        try {
            compose.waitUntil(5_000) {
                var matches = false
                scenario!!.onActivity {
                    val insets = ViewCompat.getRootWindowInsets(it.window.decorView)
                    matches = insets != null && insets.isVisible(WindowInsetsCompat.Type.ime()) == visible &&
                        (!visible || insets.getInsets(WindowInsetsCompat.Type.ime()).bottom > 0)
                }
                matches
            }
        } catch (timeout: ComposeTimeoutException) {
            throw AssertionError("$explanation: system IME insets did not reach visible=$visible; no keyboard setting was changed. ${imeDiagnostics()}", timeout)
        }
        scenario!!.onActivity {
            val insets = ViewCompat.getRootWindowInsets(it.window.decorView)
            assertNotNull("$explanation: root WindowInsets must be available", insets)
            assertEquals(explanation, visible, insets!!.isVisible(WindowInsetsCompat.Type.ime()))
        }
    }
    private fun assertTouchFocused(field: SemanticsNodeInteraction, label: String) {
        try {
            field.assertIsFocused()
        } catch (failure: AssertionError) {
            throw AssertionError("$label field did not gain focus from its real center touch. ${imeDiagnostics()}", failure)
        }
    }
    /** Only input/window state is reported: never text, usernames, credentials or other app data. */
    private fun imeDiagnostics(): String {
        val fields = runCatching {
            compose.onAllNodes(hasSetTextAction()).fetchSemanticsNodes().mapIndexed { index, node ->
                "field[$index](focused=${node.config.getOrNull(SemanticsProperties.Focused)}, " +
                    "disabled=${node.config.contains(SemanticsProperties.Disabled)}, bounds=${node.boundsInWindow})"
            }.joinToString()
        }.getOrDefault("field semantics unavailable")
        var platform = "Activity diagnostics unavailable"
        runCatching {
            scenario!!.onActivity {
                val decor = it.window.decorView
                val focus = it.currentFocus
                val config = it.resources.configuration
                val manager = it.getSystemService(InputMethodManager::class.java)
                val insets = ViewCompat.getRootWindowInsets(decor)
                val visible = Rect().also(decor::getWindowVisibleDisplayFrame)
                val selected = Settings.Secure.getString(it.contentResolver, Settings.Secure.DEFAULT_INPUT_METHOD)
                val showWithHardware = Settings.Secure.getInt(it.contentResolver, "show_ime_with_hard_keyboard", -1)
                platform = "sdk=${Build.VERSION.SDK_INT}, selectedIme=$selected, " +
                    "enabledImes=${manager.enabledInputMethodList.map { ime -> ime.id }}, " +
                    "keyboard=${config.keyboard}, keyboardHidden=${config.keyboardHidden}, hardKeyboardHidden=${config.hardKeyboardHidden}, " +
                    "showImeWithHardKeyboard=$showWithHardware, windowFocused=${decor.hasWindowFocus()}, " +
                    "viewFocus=${focus?.javaClass?.simpleName}, textEditor=${focus?.onCheckIsTextEditor()}, " +
                    "immActive=${manager.isActive}, immAcceptingText=${manager.isAcceptingText}, " +
                    "imeVisible=${insets?.isVisible(WindowInsetsCompat.Type.ime())}, " +
                    "imeBottom=${insets?.getInsets(WindowInsetsCompat.Type.ime())?.bottom}, " +
                    "softInputMode=${it.window.attributes.softInputMode}, decorSize=${decor.width}x${decor.height}, visibleFrame=$visible"
            }
        }
        return "$fields; $platform"
    }
    private fun assertFullyExpandedPanel() {
        val panel = compose.onNode(SemanticsMatcher.keyIsDefined(SemanticsProperties.PaneTitle))
        compose.waitUntil(5_000) {
            val node = panel.fetchSemanticsNode()
            kotlin.math.abs(node.size.height - node.boundsInWindow.height) <= 1f
        }
        val node = panel.fetchSemanticsNode()
        assertTrue("The complete measured panel must fit above navigation, without a clipped middle detent",
            kotlin.math.abs(node.size.height - node.boundsInWindow.height) <= 1f)
    }
    private fun assertAboveIme(button: SemanticsNodeInteraction) {
        val bounds = button.fetchSemanticsNode().boundsInWindow
        scenario!!.onActivity {
            val decor = it.window.decorView
            val insets = ViewCompat.getRootWindowInsets(decor)!!
            assertTrue("The keyboard must still be open when checking the final button", insets.isVisible(WindowInsetsCompat.Type.ime()))
            val visible = Rect()
            decor.getWindowVisibleDisplayFrame(visible)
            val windowOrigin = IntArray(2)
            decor.getLocationOnScreen(windowOrigin)
            assertTrue("The last form button must remain above the system keyboard", bounds.bottom + windowOrigin[1] <= visible.bottom + 1)
            assertTrue("The last form button must be fully reachable, not a clipped sliver",
                bounds.height >= 47 * it.resources.displayMetrics.density)
        }
    }
    private fun withModel(block: (HomeViewModel) -> Unit) {
        scenario!!.onActivity { block(ViewModelProvider(it)[HomeViewModel::class.java]) }
    }
    private fun text(english: String, chinese: String) = hasText(english) or hasText(chinese)
    private fun description(english: String, chinese: String) = hasContentDescription(english) or hasContentDescription(chinese)
    private fun tab(english: String, chinese: String) = text(english, chinese) and SemanticsMatcher.expectValue(SemanticsProperties.Role, Role.Tab)
}
