package com.example.wezterm_android

import org.junit.Assert.assertEquals
import org.junit.Test

class TerminalZoomTest {
    @Test
    fun clampsZoomPercentToSupportedRange() {
        assertEquals(TerminalZoom.MIN_PERCENT, TerminalZoom.clamp(0))
        assertEquals(100, TerminalZoom.clamp(100))
        assertEquals(TerminalZoom.MAX_PERCENT, TerminalZoom.clamp(500))
    }

    @Test
    fun cellSizeScalesWithDensityAndZoomLikeTheRenderer() {
        assertEquals(listOf(11f, 21f), TerminalZoom.cellSizePx(160, 100).toList())
        assertEquals(listOf(5.5f, 10.5f), TerminalZoom.cellSizePx(160, 50).toList())
        assertEquals(listOf(44f, 84f), TerminalZoom.cellSizePx(320, 200).toList())
    }

    @Test
    fun gridDimensionsFloorsToWholeCells() {
        assertEquals(
            listOf(10, 3),
            TerminalZoom.gridDimensions(110f, 70f, 11f, 21f).toList(),
        )
        assertEquals(
            listOf(1, 1),
            TerminalZoom.gridDimensions(5f, 5f, 11f, 21f).toList(),
        )
    }
}
