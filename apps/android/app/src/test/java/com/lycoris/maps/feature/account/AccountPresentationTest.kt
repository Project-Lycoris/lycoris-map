package com.lycoris.maps.feature.account

import com.lycoris.maps.core.model.Language
import com.lycoris.maps.core.network.ApiFailure
import org.junit.Assert.*
import org.junit.Test

class AccountPresentationTest {
    @Test fun ambiguousRegistrationExplainsPossibleCreationWithoutBlindRetry() {
        val network = accountFailureMessage(ApiFailure.Network(false), Language.EN, AccountPage.REGISTER)
        val service = accountFailureMessage(ApiFailure.Http(503), Language.ZH, AccountPage.REGISTER)
        assertTrue(network.contains("try signing in before registering again"))
        assertTrue(service.contains("账号可能已创建"))
    }

    @Test fun unsafeServiceTextIsNeverDisplayed() {
        val text = accountFailureMessage(ApiFailure.Http(500, serviceMessage = "private/raw/body"), Language.EN, AccountPage.LOGIN)
        assertFalse(text.contains("private/raw/body"))
    }

    @Test fun usernameRuleIsExplainedRatherThanCalledADuplicate() {
        val server = accountFailureMessage(ApiFailure.Http(400, serviceCode = 4005), Language.ZH, AccountPage.REGISTER)
        val local = accountFailureMessage(ApiFailure.InvalidInput("usernameAt"), Language.EN, AccountPage.REGISTER)
        assertEquals("用户名不能包含 @。", server)
        assertEquals("Usernames can't contain @.", local)
    }

    @Test fun rateLimitedPasswordChangeIsNotReportedAsUncertain() {
        val text = accountFailureMessage(ApiFailure.Http(429, serviceCode = 42934, retryAfterSeconds = 900), Language.EN, AccountPage.PASSWORD)
        assertEquals("Too many attempts. Please try again later.", text)
    }

    @Test fun avatarInitialsKeepUnicodeCodePointsWhole() {
        assertEquals("温", initials("温晓"))
        assertEquals("NS", initials("Nora Smith"))
        assertEquals("🌷", initials("🌷Nora"))
        assertEquals("L", initials("  "))
    }
}
