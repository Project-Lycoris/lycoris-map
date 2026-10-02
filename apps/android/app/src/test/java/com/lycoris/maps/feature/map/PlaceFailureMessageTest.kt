package com.lycoris.maps.feature.map

import com.lycoris.maps.core.model.Language
import com.lycoris.maps.core.network.ApiFailure
import com.lycoris.maps.core.network.apiCall
import java.net.SocketTimeoutException
import java.io.IOException
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

class PlaceFailureMessageTest {
    @Test fun errorsHaveDistinctSafeMessagesInBothLanguages() {
        val failures = listOf(ApiFailure.Network(false), ApiFailure.Network(true),
            ApiFailure.Http(401), ApiFailure.Http(403), ApiFailure.Http(404),
            ApiFailure.Http(429), ApiFailure.Http(500, serviceMessage = "private database stack"),
            ApiFailure.InvalidResponse())
        listOf(Language.ZH, Language.EN).forEach { language ->
            val messages = failures.map { it.displayMessage(language) }
            assertEquals(failures.size, messages.distinct().size)
            assertTrue(messages.none { it.contains("private database stack") })
            assertEquals(ApiFailure.Network(true).displayMessage(language),
                ApiFailure.Http(504).displayMessage(language))
        }
    }

    @Test fun networkTimeoutAndCancellationRetainTheirMeaning() = runTest {
        val timeout = runCatching { apiCall<Unit> { throw SocketTimeoutException() } }.exceptionOrNull()
        assertTrue(timeout is ApiFailure.Network && timeout.timedOut)
        val offline = runCatching { apiCall<Unit> { throw IOException() } }.exceptionOrNull()
        assertTrue(offline is ApiFailure.Network && !offline.timedOut)
        val cancellation = CancellationException("panel closed")
        val result = runCatching { apiCall<Unit> { throw cancellation } }.exceptionOrNull()
        assertSame(cancellation, result)
    }
}
