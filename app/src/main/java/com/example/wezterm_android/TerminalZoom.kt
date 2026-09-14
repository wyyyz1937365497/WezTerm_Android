package com.example.wezterm_android

import android.content.Context

/**
 * Terminal zoom state shared by the native renderer, the settings slider and
 * the grid preview. The percentage scales the terminal cell size; larger cells
 * mean fewer rows and columns (zoom in), smaller cells mean more (zoom out).
 */
internal object TerminalZoom {
    const val MIN_PERCENT = 50
    const val MAX_PERCENT = 200
    const val STEP_PERCENT = 5
    const val DEFAULT_PERCENT = 100

    private const val PREFERENCES_NAME = "display"
    private const val KEY_PERCENT = "terminal_zoom_percent"

    fun clamp(percent: Int): Int = percent.coerceIn(MIN_PERCENT, MAX_PERCENT)

    fun load(context: Context): Int =
        clamp(
            context.getSharedPreferences(PREFERENCES_NAME, Context.MODE_PRIVATE)
                .getInt(KEY_PERCENT, DEFAULT_PERCENT),
        )

    fun save(context: Context, percent: Int) {
        context.getSharedPreferences(PREFERENCES_NAME, Context.MODE_PRIVATE)
            .edit()
            .putInt(KEY_PERCENT, clamp(percent))
            .apply()
    }

    /**
     * Mirrors the native `cell_size()` math: 11 x 21 pixels at 160 dpi, scaled
     * by screen density and the zoom percentage.
     */
    fun cellSizePx(densityDpi: Int, percent: Int): FloatArray {
        val densityScale = densityDpi.coerceAtLeast(120) / 160f
        val zoom = clamp(percent) / 100f
        return floatArrayOf(11f * densityScale * zoom, 21f * densityScale * zoom)
    }

    /** Whole cell grid that fits inside the given pixel area. */
    fun gridDimensions(
        widthPx: Float,
        heightPx: Float,
        cellWidth: Float,
        cellHeight: Float,
    ): IntArray =
        intArrayOf(
            (widthPx / cellWidth).toInt().coerceAtLeast(1),
            (heightPx / cellHeight).toInt().coerceAtLeast(1),
        )
}
