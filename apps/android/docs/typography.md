# Android typography and icon assets

Roboto and its family variants are strictly prohibited in this project, including
font dependencies, primary fonts and fallback configuration. Material 3 remains
the component and type-scale system; it does not require a particular font family.

The app owns its fonts and text scale in `core/designsystem/LycorisTypography.kt`.
Interface text uses the packaged Noto Sans SC variable font for Chinese and Latin.
Weights 400, 500, 600 and 700 resolve from the same local TTF, without a font provider,
Google Play services or a font download. The platform theme uses the same font at
400, with a static Regular instance on Android 26/27 (see below). This covers
Compose, the search `BasicTextField`, and native map cluster counts. Emoji and characters outside this
font's repertoire still use Android's fallback fonts. Map-provider labels and
system UI (keyboard, permission dialogs, share sheet) remain provider/system-owned.

Before this change, the app used Material 3 `Typography()` with the ROM's generic
sans-serif family and explicit size overrides in screen files. There was no
bundled TTF/OTF. The existing numeric sizes are retained and centralized below.
Search input now shares the same text style as its placeholder.

## Screen styles

All interface sizes and line heights below are in **sp**. They are baseline design
values, not physical pixels. System accessibility font scaling is deliberately
preserved; the app does not override `fontScale` or force fixed pixel text.

| Usage | Size | Line height | Weight |
| --- | ---: | ---: | ---: |
| Panel titles: Find Nearby, Bookmarks, Settings, detail | 22 | 28 | 400 |
| Profile display name | 22 | 28 | 400 |
| Nearby cards and result titles | 17 | 22 | 600 |
| Settings / profile option rows | 17 | 24 | 400 |
| Search text and placeholder | 16 | 24 | 400 |
| Form text / default body | 16 | 24 | 400 |
| Detail description | 16 | 23 | 400 |
| Result description | 15 | 20 | 400 |
| Detail distance | 15 | 24 | 400 |
| Buttons / opening-hours labels | 14 | 20 | 500 |
| Secondary body / email / errors | 14 | 20 | 400 |
| Bottom navigation / small tags | 12 | 16 | 500 |
| Helper text | 12 | 16 | 400 |
| Map attribution | 11 | 14 | 400 |

The full pinned Material scale is in `LycorisTypography`: display 57/45/36,
headline 32/28/24, title 22/16/14, body 16/14/12 and label 14/12/11 sp.
Additional screen styles reuse it through `LycorisTextStyles`.

Native cluster numbers keep their existing map-symbol sizing: `14 * density`
pixels, reduced only when a long count would exceed the circle. They use a cached
500-weight packaged typeface shared by the MapLibre, Google and Tencent renderers.
The cache applies `wght=500` with the API 26 `Paint.setFontVariationSettings` API,
the same route used by Compose's resolver, instead of relying on the API 28-only
framework XML variation attribute. No additional medium font file is packaged.

## Font provenance

- Family: Noto Sans SC, variable TrueType, weight axis 100–900.
- Upstream: https://github.com/google/fonts/tree/a85815a42757630ce188fdad368c2dfc444d4773/ofl/notosanssc
- Original filename: `NotoSansSC[wght].ttf`; bytes are unmodified.
- App resource: `app/src/main/res/font/noto_sans_sc.ttf`.
- SHA-256: `a3041811a78c361b1de50f953c805e0244951c21c5bd412f7232ef0d899af0da`.
- Size: 17,772,300 bytes before APK compression.
- License and attribution: SIL Open Font License 1.1, packaged as
  `app/src/main/assets/licenses/NotoSansSC-OFL.txt`.
- Android's local-font API: https://developer.android.com/develop/ui/compose/text/fonts

The full upstream regional font is included rather than a subset of current UI
strings, so user-generated place names are not limited to the current interface
vocabulary. This is not a guarantee of coverage for every Unicode character.

### Android 26/27 platform Regular fallback

The source variable font's default axis and OS/2 weight are **100**, not 400.
The framework font-family XML parser only reads `android:fontVariationSettings`
from API 28. A `fontWeight="400"` declaration alone does not instantiate its
outlines at 400 on older platforms. AndroidX supports variation settings on API
26, but native framework views and `android.app.TimePickerDialog` do not reliably
use `ResourcesCompat`; the activity is a `ComponentActivity`, not AppCompat.

`res/font/lycoris_regular.xml` therefore selects the static
`lycoris_sans_sc_regular.ttf` for API 26/27. `res/font-v28/lycoris_regular.xml`
selects the original variable font with an explicit 400 axis on API 28+.
Compose still sets each weight axis explicitly on the original variable font.

The fallback is a full-glyph fixed 400 instance generated with **fontTools
4.66.1**, with family name **Lycoris Sans SC** and style **Regular**. The OFL,
copyright metadata, 31,036 glyphs and 30,890 Unicode mappings are preserved.
It is not a subset of interface text. Its modification notice is packaged as
`assets/licenses/LycorisSansSC-Regular-NOTICE.txt`; the original OFL remains
alongside it. The fallback is 10,595,904 bytes before compression; SHA-256:
`5dc9d2f258edadd45bbbbad2f74d131c113ae95cebef0547b5557bee05209882`.
To regenerate from the pinned source, use an isolated Python
environment with `fonttools==4.66.1`, from `apps/android`:

```python
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

font = TTFont("app/src/main/res/font/noto_sans_sc.ttf", recalcTimestamp=False)
font = instantiateVariableFont(font, {"wght": 400}, inplace=True, updateFontNames=True)
names = {
    1: "Lycoris Sans SC", 2: "Regular",
    3: "2.004;Lycoris;LycorisSansSC-Regular;wght400",
    4: "Lycoris Sans SC Regular", 6: "LycorisSansSC-Regular",
    16: "Lycoris Sans SC", 17: "Regular",
}
for name_id, value in names.items():
    records = [n for n in font["name"].names if n.nameID == name_id]
    for n in records:
        font["name"].setName(value, name_id, n.platformID, n.platEncID, n.langID)
    if not records:
        font["name"].setName(value, name_id, 3, 1, 0x409)
font["name"].removeNames(nameID=25)
font.save("app/src/main/res/font/lycoris_sans_sc_regular.ttf")
```

## Icons

- The 13 Figma SVGs reside in `app/src/main/assets/figma`. They contain vector
  artwork, no font-based text or remote image references.
- `FigmaIcon` and native map artwork loaders use the packaged Coil SVG decoder.
- Other Material icons are Compose `ImageVector` paths compiled into the app
  through `material-icons-extended`; they are not an installed icon font.
- Avatar images, place photos and map tiles are separate remote content.

A ROM does not need to preinstall a “MUI font” or SVG application for these icons.
Packaging evidence alone cannot identify the cause of an earlier physical-device
blank screen. `BundledTypographyTest` exercises actual local font resolution and
all SVG decodes on Android; it does not substitute for testing the affected phone.

## Verification — 2026-09-23

- 150 QA JVM tests passed; `lintQa`, QA, optimized Preview, and instrumentation
  APK builds passed.
- Both QA and R8/resource-shrunk Preview contain the exact upstream font bytes,
  the OFL notice and all 13 unchanged SVGs. The font occupies 11,276,545 compressed
  APK bytes (about 11.3 MB); Preview's resource obfuscation changes its internal
  filename, not the font bytes.
- A direct TTF cmap check covers all 394 unique Han characters, Latin letters and
  digits collected from Android Kotlin sources.
- Five targeted API 36 AOSP instrumentation cases passed across two runs:
  local font/glyph rendering at four weights, all 13 SVGs decoding to nonblank
  images, Nearby entry, account-page navigation, and fractional-density/1.3× text
  Nearby panel stability. Tests use the isolated QA package.
- Variable-font `Typeface.weight` can expose the source font's default metadata
  rather than its selected variation. The font regression therefore compares
  actual rendered stroke coverage between weights and native/Compose rendering.
- These checks do not establish acceptance on the friends' Xiaomi/realme devices.
