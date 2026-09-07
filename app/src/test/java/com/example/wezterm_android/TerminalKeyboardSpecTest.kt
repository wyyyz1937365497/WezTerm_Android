package com.example.wezterm_android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class TerminalKeyboardSpecTest {
    private val keys = TerminalKeyboardSpec.rows.flatten() + TerminalKeyboardSpec.specialKeys

    @Test
    fun exposesCompleteEnglishAlphabetInBothCases() {
        val characterKeys = keys.filter { it.normalText != null }
        val normal = characterKeys.mapNotNull { it.outputText(false) }.joinToString("")
        val shifted = characterKeys.mapNotNull { it.outputText(true) }.joinToString("")

        ('a'..'z').forEach { letter ->
            assertTrue("missing lowercase $letter", letter in normal)
            assertTrue("missing uppercase ${letter.uppercaseChar()}", letter.uppercaseChar() in shifted)
        }
    }

    @Test
    fun shiftLayerContainsCommonShellSymbols() {
        val shifted = keys.mapNotNull { it.outputText(true) }.joinToString("")
        "~!@#\$%^&*()_+{}|:\"<>?".forEach { symbol ->
            assertTrue("missing shifted symbol $symbol", symbol in shifted)
        }
    }

    @Test
    fun exposesTerminalNavigationAndFunctionKeys() {
        val actions = keys.mapNotNull { it.action }.toSet()

        assertTrue(TerminalAction.ESCAPE in actions)
        assertTrue(TerminalAction.TAB in actions)
        assertTrue(TerminalAction.ENTER in actions)
        assertTrue(TerminalAction.HOME in actions)
        assertTrue(TerminalAction.END in actions)
        assertTrue(TerminalAction.PAGE_UP in actions)
        assertTrue(TerminalAction.PAGE_DOWN in actions)
        assertTrue(TerminalAction.ARROW_LEFT in actions)
        assertTrue(TerminalAction.ARROW_DOWN in actions)
        assertTrue(TerminalAction.ARROW_UP in actions)
        assertTrue(TerminalAction.ARROW_RIGHT in actions)
        assertEquals(12, actions.count { it.name.matches(Regex("F\\d+")) })
    }

    @Test
    fun exposesOneShotModifierKeys() {
        assertEquals(
            TerminalModifier.entries.toSet(),
            keys.mapNotNull { it.modifier }.toSet(),
        )
    }

    @Test
    fun longSpecialRowIsSeparatedForRightHandGrid() {
        assertTrue(TerminalKeyboardSpec.specialKeys.size > 12)
        assertTrue(TerminalKeyboardSpec.rows.none { row ->
            row.count { it.action?.name?.matches(Regex("F\\d+")) == true } > 0
        })
    }
}
