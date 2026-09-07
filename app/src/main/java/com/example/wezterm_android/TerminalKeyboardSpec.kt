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
    private fun character(normal: String, shifted: String = normal, widthDp: Int = 46) =
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

    val specialKeys: List<TerminalKeySpec> = buildList {
        add(action("Esc", TerminalAction.ESCAPE, 58))
        add(action("Home", TerminalAction.HOME, 58))
        add(action("End", TerminalAction.END, 58))
        add(action("PgUp", TerminalAction.PAGE_UP, 58))
        add(action("PgDn", TerminalAction.PAGE_DOWN, 58))
        add(action("Ins", TerminalAction.INSERT, 58))
        add(action("Del", TerminalAction.FORWARD_DELETE, 58))
        TerminalAction.entries
            .filter { it.name.matches(Regex("F\\d+")) }
            .forEach { function -> add(action(function.name, function, 58)) }
    }

    val rows: List<List<TerminalKeySpec>> = listOf(
        listOf(
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
            action("Back", TerminalAction.BACKSPACE, 78),
        ),
        listOf(
            action("Tab", TerminalAction.TAB, 64),
            *"qwertyuiop".map { character(it.toString(), it.uppercase()) }.toTypedArray(),
            character("[", "{"),
            character("]", "}"),
            character("\\", "|"),
        ),
        listOf(
            modifier("Ctrl", TerminalModifier.CTRL),
            *"asdfghjkl".map { character(it.toString(), it.uppercase()) }.toTypedArray(),
            character(";", ":"),
            character("'", "\""),
            action("Enter", TerminalAction.ENTER, 82),
        ),
        listOf(
            modifier("Shift", TerminalModifier.SHIFT, 82),
            *"zxcvbnm".map { character(it.toString(), it.uppercase()) }.toTypedArray(),
            character(",", "<"),
            character(".", ">"),
            character("/", "?"),
            action("↑", TerminalAction.ARROW_UP, 54),
        ),
        listOf(
            modifier("Alt", TerminalModifier.ALT),
            character("Space", "Space", 260).copy(normalText = " ", shiftedText = " "),
            action("←", TerminalAction.ARROW_LEFT, 54),
            action("↓", TerminalAction.ARROW_DOWN, 54),
            action("→", TerminalAction.ARROW_RIGHT, 54),
        ),
    )
}
