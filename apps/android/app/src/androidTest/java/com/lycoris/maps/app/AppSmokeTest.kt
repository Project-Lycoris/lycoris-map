package com.lycoris.maps.app

import android.content.Context
import android.content.Intent
import android.content.SharedPreferences
import android.net.Uri
import android.os.Build
import android.speech.SpeechRecognizer
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.lifecycle.ViewModelProvider
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lycoris.maps.BuildConfig
import com.lycoris.maps.core.device.QA_LOCAL_NETWORK_REQUESTED
import com.lycoris.maps.core.data.preferences.SearchType
import com.lycoris.maps.core.model.Language
import kotlinx.coroutines.runBlocking
import com.lycoris.maps.core.model.PlaceCategory
import com.lycoris.maps.feature.map.HomeViewModel
import com.lycoris.maps.feature.map.MainSection
import com.lycoris.maps.feature.map.SecondaryPage
import org.junit.After
import org.junit.Assert.*
import org.junit.Assume.assumeFalse
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Starts the real application, Activity, ViewModel, map and device effects.
 * UI assertions do not require a running backend or successful tile downloads.
 * Only the QA package is allowed; no credentials are entered and no server writes are requested.
 * First-launch location permission is deliberately excluded from this smoke suite.
 */
@RunWith(AndroidJUnit4::class)
class AppSmokeTest {
    @get:Rule val compose = createEmptyComposeRule()

    private val context: Context get() = InstrumentationRegistry.getInstrumentation().targetContext
    private var scenario: ActivityScenario<MainActivity>? = null
    private var scenarioIntentBeforeWarmLink: Intent? = null
    private var permissionPreferences: SharedPreferences? = null
    private var originalLocationRequested: Boolean? = null
    private var originalQaLocalNetworkRequested: Boolean? = null

    @Before fun launchQaActivity() {
        assertTrue("Smoke tests require the QA build", BuildConfig.TEST_ENVIRONMENT)
        assertEquals("com.lycoris.maps.qa", context.packageName)
        assertEquals("http://10.0.2.2:18187/", BuildConfig.API_BASE_URL)
        permissionPreferences = context.getSharedPreferences("device-permissions", Context.MODE_PRIVATE).also { preferences ->
            originalLocationRequested = if (preferences.contains("location-requested")) preferences.getBoolean("location-requested", false) else null
            originalQaLocalNetworkRequested = if (preferences.contains(QA_LOCAL_NETWORK_REQUESTED)) preferences.getBoolean(QA_LOCAL_NETWORK_REQUESTED, false) else null
            // System permission prompts are a separate matrix; never grant them in UI smoke.
            assertTrue(preferences.edit().putBoolean("location-requested", true).putBoolean(QA_LOCAL_NETWORK_REQUESTED, true).commit())
        }
        // A startup exception in the real Activity/ViewModel must fail here, not be hidden by a test host.
        scenario = ActivityScenario.launch(MainActivity::class.java)
        compose.waitUntil(10_000) { compose.onAllNodes(tab("Explore", "探索")).fetchSemanticsNodes().size == 1 }
        compose.onNode(tab("Explore", "探索")).assertIsDisplayed().assertIsSelected()
    }

    @After fun closeActivityAndRestorePermissionMarker() {
        try {
            // ActivityScenario associates lifecycle events with its original launch Intent.
            // MainActivity correctly replaces that Intent in onNewIntent; restore it only for
            // teardown so Scenario can observe DESTROYED instead of ignoring the event.
            scenarioIntentBeforeWarmLink?.let { original -> scenario?.onActivity { it.intent = original } }
            scenario?.close()
        } finally {
            permissionPreferences?.edit()?.apply {
                val previous = originalLocationRequested
                if (previous == null) remove("location-requested") else putBoolean("location-requested", previous)
                val previousLocalNetwork = originalQaLocalNetworkRequested
                if (previousLocalNetwork == null) remove(QA_LOCAL_NETWORK_REQUESTED) else putBoolean(QA_LOCAL_NETWORK_REQUESTED, previousLocalNetwork)
            }?.let { assertTrue("Could not restore the QA permission marker", it.commit()) }
        }
    }

    @Test fun initialNearbyCardsOpenTheirCategoryAndCloseToExplore() {
        val categories = listOf(
            Triple("Accessible Toilets", "无障碍卫生间", PlaceCategory.ACCESSIBLE_TOILET),
            Triple("Nursing Rooms", "母婴室", PlaceCategory.BABY_ROOM),
            Triple("Medical Institutions", "医疗机构", PlaceCategory.FRIENDLY_CLINIC),
        )
        // All three are visible at the initial middle detent, without expanding first.
        categories.forEach { (english, chinese, _) -> compose.onNode(categoryCard(english, chinese)).assertIsDisplayed() }
        categories.forEach { (english, chinese, category) ->
            compose.onNode(categoryCard(english, chinese)).performClick()
            compose.onNode(heading("Nearby", "附近点位")).assertIsDisplayed()
            var radiusMeters = 0
            withModel {
                assertEquals(SecondaryPage.NEARBY, it.page.value)
                assertEquals(category, it.nearbyCategory.value)
                radiusMeters = it.preferences.value.radiusMeters
            }
            // Entering a category must show its result list, not the three category cards again.
            categories.forEach { (en, zh, _) ->
                compose.onAllNodes(categoryCard(en, zh)).assertCountEquals(0)
            }
            // The selected category and search radius appear as a short subtitle row.
            val range = if (radiusMeters % 1000 == 0) "${radiusMeters / 1000}km" else "${radiusMeters}m"
            compose.onNode(text("$english · $range", "$chinese · $range")).assertIsDisplayed()
            closePanel()
            // Closing returns to the original three-category entry point.
            compose.onNode(heading("Find Nearby", "查找附近")).assertIsDisplayed()
            categories.forEach { (en, zh, _) -> compose.onNode(categoryCard(en, zh)).assertIsDisplayed() }
            compose.onNode(tab("Explore", "探索")).assertIsSelected()
        }
    }

    @Test fun primaryNavigationSurvivesActivityRecreation() {
        compose.onNode(tab("Bookmarks", "收藏")).performClick().assertIsSelected()
        compose.onNode(heading("Bookmarks", "收藏")).assertIsDisplayed()
        withModel { assertEquals(MainSection.BOOKMARKS, it.section.value); assertNull(it.page.value) }

        compose.onNode(tab("Settings", "设置")).performClick().assertIsSelected()
        compose.onNode(text("Choose Language", "语言") and hasClickAction()).assertIsDisplayed()
        compose.onNode(text("Searching Range", "搜索范围") and hasClickAction()).assertIsDisplayed()
        compose.onNode(text("Map Source", "地图来源") and hasClickAction()).assertIsDisplayed()
        scenario!!.recreate()
        compose.onNode(tab("Settings", "设置")).assertIsDisplayed().assertIsSelected()
        withModel { assertEquals(MainSection.SETTINGS, it.section.value); assertNull(it.page.value) }

        compose.onNode(tab("Explore", "探索")).performClick().assertIsSelected()
        compose.onNode(heading("Find Nearby", "查找附近")).assertIsDisplayed()
    }

    @Test fun textSearchUsesTheRealViewModelAndBackReturnsToExplore() {
        val query = "Synthetic Android smoke query"
        compose.onNode(hasSetTextAction()).performTextInput(query)
        compose.onNode(hasSetTextAction()).performImeAction()
        compose.onNode(heading("Search results", "搜索结果")).assertIsDisplayed()
        withModel { assertEquals(query, it.query.value); assertEquals(SecondaryPage.SEARCH, it.page.value) }
        back()
        compose.onNode(heading("Find Nearby", "查找附近")).assertIsDisplayed()
        withModel { assertNull(it.page.value); assertEquals(MainSection.EXPLORE, it.section.value) }
    }

    @Test fun rangeHasOneNumericInputAndCancelPreservesTheSetting() {
        compose.onNode(tab("Settings", "设置")).performClick()
        var originalRadius = 0
        withModel { originalRadius = it.preferences.value.radiusMeters }
        compose.onNode(text("Searching Range", "搜索范围") and hasClickAction()).performClick()
        val inDialog = hasAnyAncestor(isDialog())
        compose.onAllNodes(hasSetTextAction() and inDialog).assertCountEquals(1)
        compose.onAllNodes(SemanticsMatcher.expectValue(SemanticsProperties.Role, Role.RadioButton) and inDialog).assertCountEquals(0)
        val input = compose.onNode(hasSetTextAction() and inDialog)
        input.assertTextContains(originalRadius.toString()).performTextReplacement("0")
        compose.onNode(text("Done", "完成") and hasClickAction()).assertIsNotEnabled()
        input.performTextReplacement("1500")
        compose.onNode(text("Done", "完成") and hasClickAction()).assertIsEnabled()
        compose.onNode(text("Cancel", "取消") and hasClickAction()).performClick()
        compose.onNode(isDialog()).assertDoesNotExist()
        withModel { assertEquals(originalRadius, it.preferences.value.radiusMeters) }
        // HomeScreen hides the bottom navigation while the IME inset exceeds the nav inset. Wait for
        // the real window to report the keyboard hidden before checking the tab, so an in-flight
        // close animation is not mistaken for the product keeping the keyboard up.
        awaitWindowImeHidden(5_000)
        compose.onNode(tab("Settings", "设置")).assertIsDisplayed().assertIsSelected()
    }

    /** Wait on the real window insets for the IME to become fully hidden; diagnostics are concise. */
    private fun awaitWindowImeHidden(timeoutMillis: Long) {
        try {
            compose.waitUntil(timeoutMillis) {
                var hidden = false
                scenario!!.onActivity { activity ->
                    val insets = ViewCompat.getRootWindowInsets(activity.window.decorView)
                    hidden = insets != null &&
                        !insets.isVisible(WindowInsetsCompat.Type.ime()) &&
                        insets.getInsets(WindowInsetsCompat.Type.ime()).bottom == 0
                }
                hidden
            }
        } catch (timeout: ComposeTimeoutException) {
            throw AssertionError("The window never reported the keyboard hidden. ${imeWindowDiagnostics()}", timeout)
        }
    }

    /** Reads only window/inset state on the Activity; never calls Compose test APIs on the UI thread. */
    private fun imeWindowDiagnostics(): String {
        var report = "window diagnostics unavailable"
        runCatching {
            scenario!!.onActivity { activity ->
                val decor = activity.window.decorView
                val insets = ViewCompat.getRootWindowInsets(decor)
                report = "windowFocus=${decor.hasWindowFocus()}, " +
                    "imeVisible=${insets?.isVisible(WindowInsetsCompat.Type.ime())}, " +
                    "imeBottom=${insets?.getInsets(WindowInsetsCompat.Type.ime())?.bottom}, " +
                    "rootSize=${decor.width}x${decor.height}"
            }
        }
        return report
    }

    @Test fun searchTypePersistsAcrossRecreationAndAboutShowsInstalledVersion() {
        val preferences = (context.applicationContext as LycorisApplication).container.preferences
        compose.onNode(tab("Settings", "设置")).performClick()
        val original = preferences.state.value.searchType
        val selected = if (original == SearchType.NURSING) SearchType.MEDICAL else SearchType.NURSING
        try {
            compose.onNode(text("Search Type", "搜索类型") and hasClickAction()).performScrollTo().performClick()
            val radio = SemanticsMatcher.expectValue(SemanticsProperties.Role, Role.RadioButton)
            SearchType.entries.forEach { type ->
                compose.onNode(text(type.title(Language.EN), type.title(Language.ZH)) and radio).assertIsDisplayed()
            }
            compose.onNode(text(selected.title(Language.EN), selected.title(Language.ZH)) and radio).performClick()
            compose.waitUntil(5_000) { preferences.state.value.searchType == selected }
            compose.onNode(isDialog()).assertDoesNotExist()
            scenario!!.recreate()
            compose.onNode(tab("Settings", "设置")).assertIsSelected()
            compose.onNode(text("Search Type", "搜索类型") and hasClickAction()).performScrollTo().performClick()
            compose.onNode(text(selected.title(Language.EN), selected.title(Language.ZH)) and radio).assertIsSelected()
            compose.onNode(text("Done", "完成") and hasClickAction()).performClick()
            compose.onNode(text("About Lycoris Maps", "关于 Lycoris Maps") and hasClickAction()).performScrollTo().performClick()
            compose.onNode(text("Version ${BuildConfig.VERSION_NAME}", "版本 ${BuildConfig.VERSION_NAME}"))
                .performScrollTo().assertIsDisplayed()
            compose.onNode(text(
                "A simple map for finding accessible toilets, nursing rooms, and medical institutions.",
                "一款简洁的地图，帮你找到无障碍卫生间、母婴室和医疗机构。",
            )).assertIsDisplayed()
            compose.onNode(text("Done", "完成") and hasClickAction()).performClick()
            compose.onNode(tab("Settings", "设置")).assertIsSelected()
        } finally {
            runBlocking { preferences.setSearchType(original) }
            compose.waitUntil(5_000) { preferences.state.value.searchType == original }
        }
    }

    @Test fun unavailableOnDeviceSpeechExplainsTheFallbackAndCanBeDismissed() {
        val available = Build.VERSION.SDK_INT >= 31 && SpeechRecognizer.isOnDeviceRecognitionAvailable(context)
        assumeFalse("This fallback case requires an AOSP device without an on-device recognizer", available)
        val microphonePermission = context.checkSelfPermission(android.Manifest.permission.RECORD_AUDIO)
        compose.onNode(description("Voice search", "语音搜索")).performClick()
        val message = text(
            "On-device speech input is unavailable on this device. Use text search.",
            "此设备暂不支持离线语音输入，请使用文字搜索。",
        )
        compose.waitUntil(5_000) { compose.onAllNodes(message).fetchSemanticsNodes().size == 1 }
        compose.onNode(message).assertIsDisplayed()
        // A delayed viewport/automatic-location error can arrive after the microphone tap.
        // It must not erase the explicit action's explanation before the user dismisses it.
        withModel { it.backgroundMessage("Synthetic late map failure") }
        compose.onNode(message).assertIsDisplayed()
        assertEquals(microphonePermission, context.checkSelfPermission(android.Manifest.permission.RECORD_AUDIO))
        compose.onNode(description("Dismiss message", "关闭提示")).performClick()
        compose.onNode(message).assertDoesNotExist()
        compose.onNode(hasSetTextAction()).assertIsEnabled()
        withModel { assertNull(it.page.value) }
    }

    @Test fun accountAndRegisterReturnWithoutSubmittingCredentials() {
        val accounts = (context.applicationContext as LycorisApplication).container.accounts
        compose.waitUntil(10_000) { accounts.state.value.initialized && !accounts.state.value.busy }
        assertNull("Run this smoke case in a signed-out QA package; it does not clear an existing session", accounts.state.value.user)
        compose.onNode(description("Account", "账号")).performClick()
        compose.onNode(heading("Log In", "登录")).assertIsDisplayed()
        expandPanel()
        compose.onNode(hasSetTextAction() and text("Username or Email", "账号或邮箱")).assertIsDisplayed()
        compose.onNode(text("Log In", "登录") and hasClickAction()).assertIsNotEnabled()
        compose.onNode(text("Create an Account", "创建账号") and hasClickAction()).performScrollTo().performClick()
        compose.onNode(heading("Register", "注册")).assertIsDisplayed()
        compose.onNode(hasSetTextAction() and text("Email", "邮箱")).assertExists()
        back()
        compose.onNode(heading("Log In", "登录")).assertIsDisplayed()
        expandPanel()
        compose.onNode(text("Forgot password?", "忘记密码？") and hasClickAction()).performScrollTo().assertIsDisplayed().performClick()
        compose.onNode(heading("Reset Password", "重置密码")).assertIsDisplayed()
        back()
        compose.onNode(heading("Log In", "登录")).assertIsDisplayed()
        withModel { assertEquals(SecondaryPage.ACCOUNT, it.page.value) }
        back()
        compose.onNode(heading("Find Nearby", "查找附近")).assertIsDisplayed()
        withModel { assertNull(it.page.value) }

        compose.onNode(tab("Bookmarks", "收藏")).performClick()
        compose.onNode(text("Log in", "登录") and hasClickAction()).performScrollTo().performClick()
        compose.onNode(heading("Log In", "登录")).assertIsDisplayed()
        expandPanel()
        compose.onNode(text("Forgot password?", "忘记密码？") and hasClickAction()).performScrollTo().assertIsDisplayed().performClick()
        compose.onNode(heading("Reset Password", "重置密码")).assertIsDisplayed()
        closePanel()
        compose.onNode(tab("Bookmarks", "收藏")).assertIsSelected()
        compose.onNode(heading("Bookmarks", "收藏")).assertIsDisplayed()
    }

    @Test fun coldActivityLinksOpenOnlyValidPlaces() {
        // Fresh Activity creation exercises onCreate, not OS App Links domain verification.
        // No process-cold-start or assetlinks.json verification is claimed by this case.
        val invalidLinks = listOf(
            "https://lycoris-map.com.evil.invalid/maps?markerId=2",
            "https://lycoris-map.com/maps?markerId=0",
        )
        invalidLinks.forEach { url ->
            scenario!!.close()
            scenario = ActivityScenario.launch(placeIntent(url))
            compose.onNode(heading("Find Nearby", "查找附近")).assertIsDisplayed()
            withModel { assertNull(it.page.value); assertNull(it.places.detail.value.id) }
        }
        scenario!!.close()
        scenario = ActivityScenario.launch(placeIntent("https://lycoris-map.com/maps?markerId=2"))
        assertDetail(2L)
        compose.onNode(description("Close", "关闭")).assertIsDisplayed()
        // Saved-instance restoration must retain the selected ID without reprocessing a stale intent.
        scenario!!.recreate()
        assertDetail(2L)
        closePanel()
        compose.onNode(heading("Find Nearby", "查找附近")).assertIsDisplayed()
    }

    @Test fun warmActivityLinkLeavesContributionAndIgnoresInvalidLinks() {
        withModel { model ->
            // A signed-out QA session opens Account; a retained synthetic session enters picking.
            // Do not select coordinates or submit: both transitions are local state only.
            model.contribute()
            assertTrue(model.pickingLocation.value || model.page.value == SecondaryPage.ACCOUNT)
        }
        deliverWarmLink("https://lycoris-map.com/maps?markerId=2")
        assertDetail(2L)
        withModel { assertFalse(it.pickingLocation.value) }
        val invalidLinks = listOf(
            "https://unrelated.invalid/maps?markerId=3",
            "https://lycoris-map.com/maps?markerId=-1",
            "https://lycoris-map.com/maps?markerId=9223372036854775808",
        )
        invalidLinks.forEach { url ->
            deliverWarmLink(url)
            assertDetail(2L)
            withModel { assertFalse(it.pickingLocation.value) }
        }
        closePanel()
        compose.onNode(heading("Find Nearby", "查找附近")).assertIsDisplayed()
        withModel { assertNull(it.page.value) }
    }

    private fun placeIntent(url: String) = Intent(context, MainActivity::class.java).apply {
        action = Intent.ACTION_VIEW
        data = Uri.parse(url)
    }
    private fun deliverWarmLink(url: String) {
        var original: MainActivity? = null
        scenario!!.onActivity { activity ->
            original = activity
            if (scenarioIntentBeforeWarmLink == null) scenarioIntentBeforeWarmLink = Intent(activity.intent)
            activity.startActivity(placeIntent(url).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP))
        }
        // Wait for real Intent delivery, rather than directly invoking the Activity callback.
        compose.waitUntil(5_000) {
            var delivered = false
            scenario!!.onActivity { activity ->
                assertSame("Warm links must reuse the existing singleTop Activity", original, activity)
                delivered = activity.intent.action == Intent.ACTION_VIEW && activity.intent.dataString == url
            }
            delivered
        }
        compose.waitForIdle()
    }
    private fun assertDetail(id: Long) {
        compose.waitUntil(5_000) {
            var selected = false
            withModel { selected = it.page.value == SecondaryPage.DETAIL && it.places.detail.value.id == id }
            selected
        }
        withModel { assertEquals(SecondaryPage.DETAIL, it.page.value); assertEquals(id, it.places.detail.value.id) }
    }

    private fun closePanel() { compose.onNode(description("Close", "关闭")).performClick() }
    private fun back() {
        scenario!!.onActivity { it.onBackPressedDispatcher.onBackPressed() }
        compose.waitForIdle()
    }
    private fun expandPanel() {
        compose.onNode(SemanticsMatcher.keyIsDefined(SemanticsProperties.PaneTitle))
            .performSemanticsAction(SemanticsActions.Expand) { it() }
    }
    private fun withModel(block: (HomeViewModel) -> Unit) {
        scenario!!.onActivity { block(ViewModelProvider(it)[HomeViewModel::class.java]) }
    }
    // A place row can also contain the Chinese category label, but it is not a category card.
    private fun categoryCard(english: String, chinese: String) = hasClickAction() and SemanticsMatcher("category card: $english") { node ->
        val labels = node.config.getOrNull(SemanticsProperties.Text)?.map { it.text }
        labels == listOf(english) || labels == listOf(chinese)
    }
    private fun text(english: String, chinese: String) = hasText(english) or hasText(chinese)
    private fun description(english: String, chinese: String) = hasContentDescription(english) or hasContentDescription(chinese)
    private fun heading(english: String, chinese: String) = text(english, chinese) and SemanticsMatcher.keyIsDefined(SemanticsProperties.Heading)
    private fun tab(english: String, chinese: String) = text(english, chinese) and SemanticsMatcher.expectValue(SemanticsProperties.Role, Role.Tab)
}
