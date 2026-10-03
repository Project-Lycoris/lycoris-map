package com.lycoris.maps.core.designsystem

import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Typeface
import android.view.ContextThemeWrapper
import android.widget.TextView
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.font.createFontFamilyResolver
import androidx.test.platform.app.InstrumentationRegistry
import coil3.ImageLoader
import coil3.request.ImageRequest
import coil3.request.SuccessResult
import coil3.request.allowHardware
import coil3.svg.SvgDecoder
import coil3.toBitmap
import com.lycoris.maps.R
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Test

/** Resolve the actual APK resources on Android; no downloadable fonts or network image URLs. */
class BundledTypographyTest {
    private val context get() = InstrumentationRegistry.getInstrumentation().targetContext

    @Test fun chineseAndLatinUseTheBundledFontAtEveryUiWeight() {
        val resolver = createFontFamilyResolver(context)
        val inkByWeight = mutableMapOf<Int, Long>()
        for (weight in listOf(FontWeight.Normal, FontWeight.Medium, FontWeight.SemiBold, FontWeight.Bold)) {
            val typeface = resolver.resolve(LycorisFontFamily, fontWeight = weight).value as Typeface
            assertNotEquals("Bundled font must not silently resolve to the system default", Typeface.DEFAULT, typeface)
            inkByWeight[weight.weight] = renderedInk(typeface)
            val paint = Paint().apply { this.typeface = typeface; textSize = 22f }
            for (character in "Lycoris0123456789无障碍卫生间母婴室医疗机构机场公共厕所附近点位") {
                assertTrue("Missing $character at ${weight.weight}", paint.hasGlyph(character.toString()))
            }
        }
        // Typeface.weight reports the source file's default style for variable fonts on some
        // Android versions. Compare rendered strokes instead, so ignored variation axes fail.
        inkByWeight.values.zipWithNext().forEach { (lighter, heavier) ->
            assertTrue("Each requested weight must render heavier strokes: $inkByWeight", heavier > lighter)
        }
        val medium = LycorisNativeFonts.medium(context)
        assertSame(medium, LycorisNativeFonts.medium(context))
        assertNotEquals(Typeface.DEFAULT, medium)
        assertEquals(inkByWeight.getValue(500), renderedInk(medium))
        val composeMedium = resolver.resolve(LycorisFontFamily, fontWeight = FontWeight.Medium).value as Typeface
        for (size in listOf(14f, 18.2f, 28f)) {
            assertEquals("Native cluster font must retain medium strokes at $size px",
                renderedInk(composeMedium, size), renderedInk(medium, size))
        }
    }

    @Test fun frameworkThemeAndStaticFallbackRenderRegularInsteadOfSourceThin() {
        val regular = createFontFamilyResolver(context)
            .resolve(LycorisFontFamily, fontWeight = FontWeight.Normal).value as Typeface
        var platformFont: Typeface? = null
        InstrumentationRegistry.getInstrumentation().runOnMainSync {
            platformFont = TextView(ContextThemeWrapper(context, R.style.Theme_Lycoris)).typeface
        }
        val expected = renderedInk(regular).toDouble()
        assertTrue(expected > 0)
        for (font in listOf(
            checkNotNull(platformFont),
            context.resources.getFont(R.font.lycoris_regular),
            context.resources.getFont(R.font.lycoris_sans_sc_regular),
        )) {
            assertNotEquals(Typeface.DEFAULT, font)
            // Static instancing rounds the variable outlines to integer font coordinates.
            // Allow their small rasterization difference, but never the source's Thin face.
            assertEquals("Framework and static fallback must render Regular strokes",
                expected, renderedInk(font).toDouble(), expected * 0.02)
        }
    }

    private fun renderedInk(typeface: Typeface, size: Float = 48f): Long {
        val bitmap = Bitmap.createBitmap(320, 100, Bitmap.Config.ARGB_8888)
        try {
            val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
                this.typeface = typeface
                color = Color.BLACK
                textSize = size
            }
            Canvas(bitmap).drawText("Aa医卫123", 8f, 70f, paint)
            val pixels = IntArray(bitmap.width * bitmap.height)
            bitmap.getPixels(pixels, 0, bitmap.width, 0, 0, bitmap.width, bitmap.height)
            return pixels.sumOf { (it ushr 24).toLong() }
        } finally {
            bitmap.recycle()
        }
    }

    @Test fun allFigmaIconsDecodeFromTheApkWithVisiblePixels() = runBlocking {
        val names = listOf("accessible_toilet", "baby_room", "bookmark", "contribute", "explore",
            "friendly_clinic", "layers", "locate", "location_heading", "nearby", "place_pin", "search", "settings")
        val loader = ImageLoader.Builder(context).build()
        try {
            for (name in names) {
                val result = loader.execute(ImageRequest.Builder(context)
                    .data("file:///android_asset/figma/$name.svg")
                    .decoderFactory(SvgDecoder.Factory()).size(64, 64).allowHardware(false).build())
                assertTrue("Bundled $name must decode: $result", result is SuccessResult)
                val bitmap = (result as SuccessResult).image.toBitmap()
                val pixels = IntArray(bitmap.width * bitmap.height)
                bitmap.getPixels(pixels, 0, bitmap.width, 0, 0, bitmap.width, bitmap.height)
                assertTrue("Bundled $name must not be blank", pixels.any { it ushr 24 != 0 })
            }
        } finally {
            loader.shutdown()
        }
    }
}
