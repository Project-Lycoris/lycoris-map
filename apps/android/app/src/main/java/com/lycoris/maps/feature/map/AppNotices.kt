package com.lycoris.maps.feature.map

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/** A late map/location refresh must not hide the result of the user's explicit action. */
internal class AppNotices {
    private val mutable = MutableStateFlow<String?>(null)
    val state: StateFlow<String?> = mutable.asStateFlow()
    private var hasActionNotice = false

    @Synchronized
    fun showAction(value: String?) {
        hasActionNotice = value != null
        mutable.value = value
    }

    @Synchronized
    fun showBackground(value: String?) {
        if (!hasActionNotice) mutable.value = value
    }
}
