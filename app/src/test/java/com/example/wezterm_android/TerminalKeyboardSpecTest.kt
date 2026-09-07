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
    fun specialKeysAreSeparatedIntoACompactRightHandPanel() {
        assertTrue(TerminalKeyboardSpec.specialKeys.size > 12)
        assertTrue(TerminalKeyboardSpec.rows.none { row ->
            row.count { it.action?.name?.matches(Regex("F\\d+")) == true } > 0
        })
        assertTrue(TerminalKeyboardSpec.specialRows.all { row -> row.size == 6 })
    }

    @Test
    fun functionKeysUseThreeOrderedGroupsOfFour() {
        val expected = (1..12).map { "F$it" }
        assertEquals(3, TerminalKeyboardSpec.functionRows.size)
        assertTrue(TerminalKeyboardSpec.functionRows.all { row -> row.size == 4 })
        assertEquals(
            expected,
            TerminalKeyboardSpec.functionRows.flatten().map { it.action?.name },
        )
    }

    @Test
    fun navigationAndArrowKeysStayInRecognizableClusters() {
        assertEquals(
            listOf(TerminalAction.INSERT, TerminalAction.HOME, TerminalAction.PAGE_UP),
            TerminalKeyboardSpec.navigationRows[0].map { it.action },
        )
        assertEquals(
            listOf(TerminalAction.FORWARD_DELETE, TerminalAction.END, TerminalAction.PAGE_DOWN),
            TerminalKeyboardSpec.navigationRows[1].map { it.action },
        )
        assertEquals(
            listOf(null, TerminalAction.ARROW_UP, null),
            TerminalKeyboardSpec.arrowRows[0].map { it?.action },
        )
        assertEquals(
            listOf(
                TerminalAction.ARROW_LEFT,
                TerminalAction.ARROW_DOWN,
                TerminalAction.ARROW_RIGHT,
            ),
            TerminalKeyboardSpec.arrowRows[1].map { it?.action },
        )
    }

    @Test
    fun modifiersFollowTheMainKeyboardRows() {
        assertEquals(TerminalAction.ESCAPE, TerminalKeyboardSpec.rows.first().first().action)
        assertEquals(
            listOf(TerminalModifier.CTRL, TerminalModifier.ALT),
            TerminalKeyboardSpec.rows.last().mapNotNull { it.modifier },
        )
        assertEquals(
            TerminalModifier.SHIFT,
            TerminalKeyboardSpec.rows[TerminalKeyboardSpec.rows.lastIndex - 1]
                .first()
                .modifier,
        )
    }
}
