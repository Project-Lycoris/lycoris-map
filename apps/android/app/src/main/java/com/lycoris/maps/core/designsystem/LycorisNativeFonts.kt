package com.lycoris.maps.core.designsystem

import android.content.Context
import android.graphics.Paint
import android.graphics.Typeface
import com.lycoris.maps.R

/** Shares the packaged medium face across native map renderers without retaining a Context. */
internal object LycorisNativeFonts {
    private var mediumTypeface: Typeface? = null

    @Synchronized
    fun medium(context: Context): Typeface = mediumTypeface
        ?: Paint().apply {
            typeface = context.applicationContext.resources.getFont(R.font.noto_sans_sc)
            // Paint applies variation axes from API 26, as Compose's font resolver does.
            // The framework font-family XML attribute only works from API 28.
            check(setFontVariationSettings("'wght' 500")) { "Bundled font does not support medium weight" }
        }.typeface.also { mediumTypeface = it }
}
