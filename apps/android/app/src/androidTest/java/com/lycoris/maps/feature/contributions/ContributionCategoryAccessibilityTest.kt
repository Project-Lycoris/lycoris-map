package com.lycoris.maps.feature.contributions

import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.unit.dp
import androidx.test.platform.app.InstrumentationRegistry
import com.lycoris.maps.app.LycorisApplication
import com.lycoris.maps.core.designsystem.LycorisTheme
import com.lycoris.maps.core.model.Language
import com.lycoris.maps.core.model.PlaceCategory
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test

/** Synthetic form state only: no sign-in, draft persistence or backend mutations. */
class ContributionCategoryAccessibilityTest {
    @get:Rule val compose = createAndroidComposeRule<ComponentActivity>()
    private val checkbox = SemanticsMatcher.expectValue(SemanticsProperties.Role, Role.Checkbox)

    @Test fun productionFormExposesFourNamedCheckboxesInBothLanguages() {
        val app = InstrumentationRegistry.getInstrumentation().targetContext.applicationContext as LycorisApplication
        val language = mutableStateOf(Language.EN)
        val draft = ContributionDraft(
            id = "00000000-0000-4000-8000-000000000001", owner = "accessibility-fixture",
            origin = "http://10.0.2.2:18187/", latitude = 31.2, longitude = 121.5,
            fields = ContributionFields(title = "Synthetic accessibility fixture"),
        )
        compose.setContent {
            LycorisTheme {
                Column(Modifier.verticalScroll(rememberScrollState())) {
                    ContributionPanel(draft, language.value, {}, app.container.contributions)
                }
            }
        }
        assertCheckboxNames(setOf("Accessible toilet", "Nursing room", "Medical institution", "Other"))
        compose.runOnIdle { language.value = Language.ZH }
        assertCheckboxNames(setOf("无障碍卫生间", "母婴室", "医疗机构", "其他"))
    }

    @Test fun tappingCategoryLabelTogglesOnceAndRetainsAtLeastOneCategory() {
        var changes = 0
        val fields = mutableStateOf(ContributionFields(title = "Fixture", venueType = "metro"))
        rows(fields = { fields.value }, onChange = { fields.value = it; changes++ })

        // A touch on the text, outside the visual checkbox, must activate the named row.
        compose.onNodeWithText("Nursing room", useUnmergedTree = true).performTouchInput { click() }
        compose.onNode(checkbox and hasText("Nursing room")).assertIsOn()
        compose.runOnIdle {
            assertEquals(listOf("accessible_toilet", "baby_room"), fields.value.selectedCategories)
            assertEquals("metro", fields.value.venueType)
            assertEquals(1, changes)
        }
        compose.onNode(checkbox and hasText("Accessible toilet")).performClick().assertIsOff()
        compose.runOnIdle {
            assertEquals(listOf("baby_room"), fields.value.selectedCategories)
            assertNull(fields.value.venueType)
            assertEquals(2, changes)
        }
        compose.onNode(checkbox and hasText("Nursing room")).performClick().assertIsOn()
        compose.runOnIdle { assertEquals(listOf("baby_room"), fields.value.selectedCategories) }
    }

    @Test fun makingACategoryPrimaryIsIndependentOfItsCheckbox() {
        var changes = 0
        val fields = mutableStateOf(ContributionFields(title = "Fixture", venueType = "metro")
            .withCategories(listOf("accessible_toilet", "baby_room")))
        rows(fields = { fields.value }, onChange = { fields.value = it; changes++ })

        compose.onNodeWithContentDescription("Make Nursing room primary")
            .assert(SemanticsMatcher.expectValue(SemanticsProperties.Role, Role.Button))
            .assertIsEnabled().performClick()
        compose.onNode(checkbox and hasText("Accessible toilet")).assertIsOn()
        compose.onNode(checkbox and hasText("Nursing room")).assertIsOn()
        compose.onNodeWithContentDescription("Make Accessible toilet primary").assertIsEnabled()
        compose.onNodeWithContentDescription("Make Nursing room primary").assertDoesNotExist()
        compose.runOnIdle {
            assertEquals(listOf("baby_room", "accessible_toilet"), fields.value.selectedCategories)
            assertEquals("metro", fields.value.venueType)
            assertEquals(1, changes)
        }
    }

    @Test fun frozenDraftDisablesCategoryAndPrimaryActions() {
        var changes = 0
        val fields = ContributionFields(title = "Fixture")
            .withCategories(listOf("accessible_toilet", "baby_room"))
        rows(fields = { fields }, enabled = false, onChange = { changes++ })

        for (label in listOf("Accessible toilet", "Nursing room", "Medical institution", "Other")) {
            compose.onNode(checkbox and hasText(label)).assertIsNotEnabled()
        }
        compose.onNodeWithText("Nursing room", useUnmergedTree = true).performTouchInput { click() }
        compose.onNodeWithContentDescription("Make Nursing room primary")
            .assertIsNotEnabled().performTouchInput { click() }
        compose.onNode(checkbox and hasText("Nursing room")).assertIsOn()
        compose.runOnIdle { assertEquals(0, changes) }
    }

    private fun rows(
        fields: () -> ContributionFields,
        enabled: Boolean = true,
        onChange: (ContributionFields) -> Unit,
    ) {
        compose.setContent {
            LycorisTheme {
                Column(Modifier.padding(30.dp)) {
                    PlaceCategory.entries.forEach { category ->
                        ContributionCategoryRow(category, fields(), false, enabled, onChange)
                    }
                }
            }
        }
    }

    private fun assertCheckboxNames(expected: Set<String>) {
        val nodes = compose.onAllNodes(checkbox).fetchSemanticsNodes()
        assertEquals(4, nodes.size)
        val labels = nodes.map { node ->
            val text = node.config.getOrNull(SemanticsProperties.Text).orEmpty().map { it.text }
            val descriptions = node.config.getOrNull(SemanticsProperties.ContentDescription).orEmpty()
            (text + descriptions).filter(String::isNotBlank)
        }
        assertTrue("Every actionable checkbox must be named: $labels", labels.all { it.size == 1 })
        assertEquals(expected, labels.flatten().toSet())
    }
}
