package com.example.wezterm_android

import org.junit.Assert.assertEquals
import org.junit.Test

class TerminalScrollAccumulatorTest {
    @Test
    fun oneFingerDistanceUsesNaturalRemoteWheelDirection() {
        val accumulator = TerminalScrollAccumulator(20f)

        assertEquals(0, accumulator.consume(12f, TerminalScrollDestination.REMOTE))
        assertEquals(1, accumulator.consume(12f, TerminalScrollDestination.REMOTE))
        assertEquals(-1, accumulator.consume(-28f, TerminalScrollDestination.REMOTE))
    }

    @Test
    fun twoFingerDistanceUsesDirectHistoryDirection() {
        val accumulator = TerminalScrollAccumulator(20f)

        assertEquals(2, accumulator.consume(-40f, TerminalScrollDestination.HISTORY))
        assertEquals(-1, accumulator.consume(20f, TerminalScrollDestination.HISTORY))
    }

    @Test
    fun remoteAndHistoryRemaindersAreIndependentAndResettable() {
        val accumulator = TerminalScrollAccumulator(20f)

        assertEquals(0, accumulator.consume(15f, TerminalScrollDestination.REMOTE))
        assertEquals(0, accumulator.consume(-15f, TerminalScrollDestination.HISTORY))
        assertEquals(1, accumulator.consume(5f, TerminalScrollDestination.REMOTE))
        assertEquals(1, accumulator.consume(-5f, TerminalScrollDestination.HISTORY))

        accumulator.reset()
        assertEquals(0, accumulator.consume(5f, TerminalScrollDestination.REMOTE))
        assertEquals(0, accumulator.consume(-5f, TerminalScrollDestination.HISTORY))
    }
}
