package com.example.wezterm_android

internal enum class TerminalScrollDestination {
    REMOTE,
    HISTORY,
}

/**
 * Converts touch pixels into discrete terminal wheel/row steps while keeping
 * independent remainders for the one- and two-finger gesture contracts.
 */
internal class TerminalScrollAccumulator(
    private val stepPixels: Float,
) {
    private var remoteRemainder = 0f
    private var historyRemainder = 0f

    init {
        require(stepPixels > 0f && stepPixels.isFinite())
    }

    fun reset() {
        remoteRemainder = 0f
        historyRemainder = 0f
    }

    fun consume(distanceY: Float, destination: TerminalScrollDestination): Int {
        if (!distanceY.isFinite()) return 0
        val directedDistance = when (destination) {
            // GestureDetector distanceY is positive while the finger moves up;
            // that is a conventional wheel-down gesture for the remote app.
            TerminalScrollDestination.REMOTE -> distanceY
            // Dragging two fingers down reveals older local history.
            TerminalScrollDestination.HISTORY -> -distanceY
        }
        val remainder = when (destination) {
            TerminalScrollDestination.REMOTE -> remoteRemainder + directedDistance
            TerminalScrollDestination.HISTORY -> historyRemainder + directedDistance
        }
        val steps = (remainder / stepPixels).toInt()
        val nextRemainder = remainder - steps * stepPixels
        when (destination) {
            TerminalScrollDestination.REMOTE -> remoteRemainder = nextRemainder
            TerminalScrollDestination.HISTORY -> historyRemainder = nextRemainder
        }
        return steps
    }
}
