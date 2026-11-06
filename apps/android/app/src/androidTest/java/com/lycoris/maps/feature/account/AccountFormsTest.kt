package com.lycoris.maps.feature.account

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Surface
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.StateRestorationTester
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.performTextReplacement
import androidx.compose.ui.unit.dp
import com.lycoris.maps.core.designsystem.LycorisTheme
import com.lycoris.maps.core.model.Language
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test

class AccountFormsTest {
    @get:Rule val compose = createComposeRule()

    @Test fun loginWaitsForCredentialsAndDisablesDuplicateSubmission() {
        val busy = mutableStateOf(false)
        var submitted = 0
        compose.setContent {
            LycorisTheme { Surface { Column(Modifier.padding(30.dp)) {
                LoginForm(Language.EN, busy.value, { _, _ -> submitted++; busy.value = true }, {})
            } } }
        }
        compose.onNodeWithText("Log In").assertIsNotEnabled()
        compose.onNode(hasSetTextAction() and hasText("Username or Email")).performTextInput("synthetic")
        compose.onNode(hasSetTextAction() and hasText("Password")).performTextInput("synthetic-password")
        compose.onNodeWithText("Log In").assertIsEnabled().performClick()
        compose.onNodeWithText("Log In").assertIsNotEnabled()
        compose.runOnIdle { assertEquals(1, submitted) }
    }

    @Test fun passwordIsMaskedAndNotRestoredAfterRecreation() {
        val restoration = StateRestorationTester(compose)
        restoration.setContent {
            LycorisTheme { Surface { Column(Modifier.padding(30.dp)) { LoginForm(Language.EN, false, { _, _ -> }, {}) } } }
        }
        val field = hasSetTextAction() and hasText("Password")
        compose.onNode(field).performTextInput("synthetic-secret")
        compose.onNode(field and SemanticsMatcher.keyIsDefined(SemanticsProperties.Password)).assertExists()
        restoration.emulateSavedInstanceStateRestore()
        assertEquals("", compose.onNode(field).fetchSemanticsNode().config[SemanticsProperties.EditableText].text)
    }

    @Test fun changingPasswordRequiresMatchingConfirmation() {
        var submitted = 0
        compose.setContent {
            LycorisTheme { Surface { Column(Modifier.padding(30.dp)) { PasswordForm(Language.EN, false) { _, _ -> submitted++ } } } }
        }
        compose.onNode(hasSetTextAction() and hasText("Current Password")).performTextInput("old-synthetic")
        compose.onNode(hasSetTextAction() and hasText("New Password")).performTextInput("new-synthetic")
        compose.onNode(hasSetTextAction() and hasText("Confirm New Password")).performTextInput("different")
        compose.onNodeWithText("Change Password").assertIsNotEnabled()
        compose.onNodeWithText("The new passwords do not match.").assertExists()
        compose.runOnIdle { assertEquals(0, submitted) }
    }

    @Test fun registrationCanReturnToLoginWithChineseLabels() {
        var returned = false
        compose.setContent {
            LycorisTheme { Surface { Column(Modifier.padding(30.dp)) {
                RegisterForm(Language.ZH, false, { _, _, _, _, _ -> }, { returned = true }, onSendCode = {})
            } } }
        }
        compose.onNode(hasSetTextAction() and hasText("邮箱")).assertExists()
        compose.onNodeWithText("注册").assertIsNotEnabled()
        compose.onNodeWithText("已有账号？登录").performClick()
        compose.runOnIdle { assertEquals(true, returned) }
    }

    @Test fun registrationExplainsTheUsernameRuleBeforeSubmitting() {
        var submitted = 0
        compose.setContent {
            LycorisTheme { Surface { Column(Modifier.padding(30.dp)) {
                RegisterForm(Language.EN, false, { _, _, _, _, _ -> submitted++ }, {}, onSendCode = {})
            } } }
        }
        val username = hasSetTextAction() and hasText("Username")
        compose.onNode(username).performTextInput("victim@example.test")
        compose.onNode(hasSetTextAction() and hasText("Email")).performTextInput("new@example.test")
        compose.onNode(hasSetTextAction() and hasText("Password")).performTextInput("new-password")
        compose.onNode(hasSetTextAction() and hasText("Verification Code")).performTextInput("123456")
        compose.onNodeWithText("Usernames can't contain @.").assertExists()
        compose.onNodeWithText("Register").assertIsNotEnabled()
        compose.onNode(username).performTextReplacement("victim")
        compose.onNodeWithText("Usernames can't contain @.").assertDoesNotExist()
        compose.onNodeWithText("Register").assertIsEnabled().performClick()
        compose.runOnIdle { assertEquals(1, submitted) }
    }
}
