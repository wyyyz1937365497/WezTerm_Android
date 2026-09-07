package com.example.wezterm_android

internal enum class TerminalModifier {
    SHIFT,
    CTRL,
    ALT,
}

internal enum class TerminalAction {
    ESCAPE,
    TAB,
    ENTER,
    BACKSPACE,
    FORWARD_DELETE,
    INSERT,
    HOME,
    END,
    PAGE_UP,
    PAGE_DOWN,
    ARROW_LEFT,
    ARROW_DOWN,
    ARROW_UP,
    ARROW_RIGHT,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

/** A platform-independent description of one key on the in-app keyboard. */
internal data class TerminalKeySpec(
    val label: String,
    val normalText: String? = null,
    val shiftedText: String? = normalText,
    val action: TerminalAction? = null,
    val modifier: TerminalModifier? = null,
    val widthDp: Int = 46,
) {
    init {
        val roles = listOf(normalText != null, action != null, modifier != null).count { it }
        require(roles == 1) { "a terminal key must have exactly one role" }
    }

    fun outputText(shift: Boolean): String? =
        if (normalText == null) null else if (shift) shiftedText else normalText
}

internal object TerminalKeyboardSpec {
    private fun character(normal: String, shifted: String = normal, widthDp: Int = 42) =
        TerminalKeySpec(
            label = if (normal == shifted) normal else "$normal\n$shifted",
            normalText = normal,
            shiftedText = shifted,
            widthDp = widthDp,
        )

    private fun action(label: String, action: TerminalAction, widthDp: Int = 58) =
        TerminalKeySpec(label = label, action = action, widthDp = widthDp)

    private fun modifier(label: String, modifier: TerminalModifier, widthDp: Int = 72) =
        TerminalKeySpec(label = label, modifier = modifier, widthDp = widthDp)

    val functionRows: List<List<TerminalKeySpec>> =
        TerminalAction.entries
            .filter { it.name.matches(Regex("F\\d+")) }
            .map { function -> action(function.name, function, 54) }
            .chunked(4)

    val navigationRows: List<List<TerminalKeySpec>> = listOf(
        listOf(
            action("Ins", TerminalAction.INSERT, 54),
            action("Home", TerminalAction.HOME, 54),
            action("PgUp", TerminalAction.PAGE_UP, 54),
        ),
        listOf(
            action("Del", TerminalAction.FORWARD_DELETE, 54),
            action("End", TerminalAction.END, 54),
            action("PgDn", TerminalAction.PAGE_DOWN, 54),
        ),
    )

    // A null reserves a key-sized slot so the arrows form a familiar inverted T.
    val arrowRows: List<List<TerminalKeySpec?>> = listOf(
        listOf(null, action("↑", TerminalAction.ARROW_UP, 54), null),
        listOf(
            action("←", TerminalAction.ARROW_LEFT, 54),
            action("↓", TerminalAction.ARROW_DOWN, 54),
            action("→", TerminalAction.ARROW_RIGHT, 54),
        ),
    )

    val specialRows: List<List<TerminalKeySpec?>> = listOf(
        listOf(null) + functionRows[0] + listOf(null),
        listOf(null) + functionRows[1] + listOf(null),
        listOf(null) + functionRows[2] + listOf(null),
        navigationRows[0] + arrowRows[0],
        navigationRows[1] + arrowRows[1],
    )

    val specialKeys: List<TerminalKeySpec> = specialRows.flatten().filterNotNull()

    val rows: List<List<TerminalKeySpec>> = listOf(
        listOf(
            action("Esc", TerminalAction.ESCAPE, 54),
            character("`", "~"),
            character("1", "!"),
            character("2", "@"),
            character("3", "#"),
            character("4", "\$"),
            character("5", "%"),
            character("6", "^"),
            character("7", "&"),
            character("8", "*"),
            character("9", "("),
            character("0", ")"),
            character("-", "_"),
            character("=", "+"),
            action("Back", TerminalAction.BACKSPACE, 74),
        ),
        listOf(
            action("Tab", TerminalAction.TAB, 60),
            *"qwertyuiop".map { character(it.toString(), it.uppercase()) }.toTypedArray(),
            character("[", "{"),
            character("]", "}"),
            character("\\", "|"),
        ),
        listOf(
            *"asdfghjkl".map { character(it.toString(), it.uppercase()) }.toTypedArray(),
            character(";", ":"),
            character("'", "\""),
            action("Enter", TerminalAction.ENTER, 78),
        ),
        listOf(
            modifier("Shift", TerminalModifier.SHIFT, 78),
            *"zxcvbnm".map { character(it.toString(), it.uppercase()) }.toTypedArray(),
            character(",", "<"),
            character(".", ">"),
            character("/", "?"),
        ),
        listOf(
            modifier("Ctrl", TerminalModifier.CTRL, 68),
            modifier("Alt", TerminalModifier.ALT, 68),
            character("Space", "Space", 300).copy(normalText = " ", shiftedText = " "),
        ),
    )
}
