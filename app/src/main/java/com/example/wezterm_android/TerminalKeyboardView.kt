package com.example.wezterm_android

import android.content.Context
import android.content.res.ColorStateList
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.text.InputType
import android.view.Gravity
import android.view.KeyEvent
import android.view.View
import android.view.ViewGroup
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputMethodManager
import android.widget.Button
import android.widget.EditText
import android.widget.HorizontalScrollView
import android.widget.LinearLayout
import android.widget.TextView
import android.widget.GridLayout

/**
 * A terminal-specific keyboard that never invokes the Android IME for its
 * English/symbol keys. The IME is attached only to [composerInput], and the
 * final committed string is sent to the PTY as one UTF-8 payload.
 */
internal class TerminalKeyboardView(context: Context) : LinearLayout(context) {
    var onKeyRequested: ((keyCode: Int, unicodeCodePoint: Int, metaState: Int) -> Unit)? = null
    var onTextRequested: ((String) -> String?)? = null
    var onInputError: ((String) -> Unit)? = null
    var onInputSent: ((Int) -> Unit)? = null
    var onDismissRequested: (() -> Unit)? = null
    var onTerminalFocusRequested: (() -> Unit)? = null

    private val terminalButtons = mutableListOf<Button>()
    private val modifierButtons = mutableMapOf<TerminalModifier, Button>()
    private val chineseButton: Button
    private val composerRow: LinearLayout
    private val composerInput: EditText
    private val sendButton: Button
    private val keyboardBody: View

    private var terminalReady: Boolean? = null
    private val activeModifiers = mutableSetOf<TerminalModifier>()

    init {
        orientation = VERTICAL
        gravity = Gravity.CENTER_HORIZONTAL
        setPadding(dp(7), dp(5), dp(7), dp(7))
        elevation = 0f
        background = roundedBackground(PANEL_COLOR, 0, BORDER_COLOR)
        contentDescription = context.getString(R.string.keyboard_panel_description)

        val header = LinearLayout(context).apply {
            orientation = HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(dp(3), 0, dp(1), dp(3))
        }
        header.addView(
            TextView(context).apply {
                text = context.getString(R.string.keyboard_title)
                setTextColor(TEXT_MUTED)
                textSize = 11f
                typeface = Typeface.create(Typeface.MONOSPACE, Typeface.BOLD)
            },
            LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f),
        )
        chineseButton = headerButton(context.getString(R.string.keyboard_chinese_input)).apply {
            setOnClickListener { toggleComposer() }
        }
        header.addView(chineseButton)
        header.addView(headerButton(context.getString(R.string.keyboard_hide)).apply {
            setOnClickListener { onDismissRequested?.invoke() }
        })
        addView(header, LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(36)))

        composerInput = EditText(context).apply {
            hint = context.getString(R.string.keyboard_composer_hint)
            setTextColor(Color.WHITE)
            setHintTextColor(TEXT_MUTED)
            textSize = 14f
            isSingleLine = true
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_CAP_SENTENCES
            imeOptions =
                EditorInfo.IME_ACTION_DONE or
                    EditorInfo.IME_FLAG_NO_EXTRACT_UI or
                    EditorInfo.IME_FLAG_NO_FULLSCREEN
            setPadding(dp(10), 0, dp(10), 0)
            backgroundTintList = ColorStateList.valueOf(ACCENT_COLOR)
            setOnEditorActionListener { _, actionId, event ->
                val requested =
                    actionId == EditorInfo.IME_ACTION_DONE ||
                        event?.keyCode == KeyEvent.KEYCODE_ENTER
                if (requested) sendComposedText()
                requested
            }
        }
        sendButton = headerButton(context.getString(R.string.keyboard_send)).apply {
            setOnClickListener { sendComposedText() }
        }
        composerRow = LinearLayout(context).apply {
            orientation = HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            visibility = View.GONE
            addView(composerInput, LayoutParams(0, dp(44), 1f))
            addView(sendButton, LayoutParams(dp(72), dp(38)).withMargins(4, 3, 0, 3))
        }
        addView(composerRow, LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(48)))

        keyboardBody = createKeyboardBody()
        addView(keyboardBody)
        setTerminalReady(false)
    }

    fun setTerminalReady(ready: Boolean) {
        if (terminalReady == ready) return
        terminalReady = ready
        terminalButtons.forEach { button ->
            button.isEnabled = ready
            button.alpha = if (ready) 1f else 0.38f
        }
        chineseButton.isEnabled = ready
        chineseButton.alpha = if (ready) 1f else 0.38f
        composerInput.isEnabled = ready
        sendButton.isEnabled = ready
        if (!ready) {
            activeModifiers.clear()
            updateModifierButtons()
            closeComposer()
        }
    }

    fun showPanel() {
        visibility = View.VISIBLE
    }

    fun hidePanel() {
        closeComposer()
        activeModifiers.clear()
        updateModifierButtons()
        visibility = View.GONE
        onTerminalFocusRequested?.invoke()
    }

    fun togglePanel() {
        if (visibility == View.VISIBLE) hidePanel() else showPanel()
    }

    fun isPanelVisible(): Boolean = visibility == View.VISIBLE

    private fun createKeyboardBody(): View {
        val mainKeys = LinearLayout(context).apply {
            orientation = VERTICAL
            gravity = Gravity.CENTER_HORIZONTAL
        }
        TerminalKeyboardSpec.rows.forEach { specs ->
            mainKeys.addView(createKeyRow(specs))
        }

        val specialKeys = GridLayout(context).apply {
            columnCount = SPECIAL_COLUMN_COUNT
            rowCount = TerminalKeyboardSpec.specialRows.size
            setPadding(dp(6), 0, 0, 0)
        }
        TerminalKeyboardSpec.specialRows.forEachIndexed { rowIndex, row ->
            check(row.size == SPECIAL_COLUMN_COUNT)
            row.forEachIndexed { columnIndex, spec ->
                val keyOrSpacer = spec?.let(::keyButton) ?: View(context)
                specialKeys.addView(
                    keyOrSpacer,
                    GridLayout.LayoutParams(
                        GridLayout.spec(rowIndex),
                        GridLayout.spec(columnIndex),
                    ).apply {
                        width = dp(SPECIAL_KEY_WIDTH_DP)
                        height = dp(KEY_HEIGHT_DP)
                        val leftMargin = if (rowIndex >= 3 && columnIndex == 3) 8 else 2
                        setMargins(dp(leftMargin), dp(2), dp(2), dp(2))
                    },
                )
            }
        }

        val body = LinearLayout(context).apply {
            orientation = HORIZONTAL
            gravity = Gravity.TOP
            minimumWidth = dp(KEYBOARD_CONTENT_MIN_WIDTH_DP)
            addView(
                mainKeys,
                LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f),
            )
            addView(
                specialKeys,
                LayoutParams(
                    ViewGroup.LayoutParams.WRAP_CONTENT,
                    ViewGroup.LayoutParams.WRAP_CONTENT,
                ),
            )
        }

        return HorizontalScrollView(context).apply {
            isHorizontalScrollBarEnabled = false
            isFillViewport = true
            overScrollMode = View.OVER_SCROLL_NEVER
            addView(
                body,
                ViewGroup.LayoutParams(
                    ViewGroup.LayoutParams.WRAP_CONTENT,
                    ViewGroup.LayoutParams.WRAP_CONTENT,
                ),
            )
        }
    }

    private fun createKeyRow(specs: List<TerminalKeySpec>): View {
        val row = LinearLayout(context).apply {
            orientation = HORIZONTAL
            gravity = Gravity.CENTER
        }
        specs.forEach { spec ->
            val button = keyButton(spec)
            row.addView(
                button,
                LayoutParams(dp(spec.widthDp), dp(KEY_HEIGHT_DP)).withMargins(2, 2, 2, 2),
            )
        }
        return row
    }

    private fun keyButton(spec: TerminalKeySpec): Button =
        Button(context).apply {
            text = spec.label
            textSize = if ('\n' in spec.label) 9f else 10.5f
            setTextColor(Color.WHITE)
            setAllCaps(false)
            minHeight = 0
            minimumHeight = 0
            minWidth = 0
            minimumWidth = 0
            setPadding(dp(2), 0, dp(2), 0)
            gravity = Gravity.CENTER
            isFocusable = false
            backgroundTintList = ColorStateList.valueOf(KEY_COLOR)
            contentDescription = spec.label.replace('\n', ' ')
            setOnClickListener { press(spec) }
            terminalButtons += this
            spec.modifier?.let { modifier -> modifierButtons[modifier] = this }
        }

    private fun headerButton(label: String): Button =
        Button(context).apply {
            text = label
            textSize = 10.5f
            setTextColor(Color.WHITE)
            setAllCaps(false)
            minHeight = 0
            minimumHeight = 0
            minWidth = 0
            minimumWidth = 0
            isFocusable = false
            setPadding(dp(10), 0, dp(10), 0)
            backgroundTintList = ColorStateList.valueOf(KEY_COLOR)
        }

    private fun press(spec: TerminalKeySpec) {
        if (terminalReady != true) return
        spec.modifier?.let { modifier ->
            if (!activeModifiers.add(modifier)) activeModifiers.remove(modifier)
            updateModifierButtons()
            return
        }

        val shift = TerminalModifier.SHIFT in activeModifiers
        val metaState =
            (if (TerminalModifier.CTRL in activeModifiers) KeyEvent.META_CTRL_ON else 0) or
                (if (TerminalModifier.ALT in activeModifiers) KeyEvent.META_ALT_ON else 0)
        val text = spec.outputText(shift)
        if (text != null) {
            onKeyRequested?.invoke(KeyEvent.KEYCODE_UNKNOWN, text.codePointAt(0), metaState)
        } else {
            val action = checkNotNull(spec.action)
            onKeyRequested?.invoke(action.androidKeyCode(), 0, metaState)
        }

        // Mobile modifier keys are one-shot. This prevents a forgotten Ctrl or
        // Alt latch from corrupting the next command.
        activeModifiers.clear()
        updateModifierButtons()
    }

    private fun toggleComposer() {
        if (terminalReady != true) return
        if (composerRow.visibility == View.VISIBLE) {
            closeComposer()
            onTerminalFocusRequested?.invoke()
            return
        }
        keyboardBody.visibility = View.GONE
        composerRow.visibility = View.VISIBLE
        composerInput.requestFocus()
        composerInput.setSelection(composerInput.text.length)
        composerInput.post {
            inputMethodManager().showSoftInput(composerInput, 0)
        }
    }

    private fun sendComposedText() {
        if (terminalReady != true) return
        val value = composerInput.text.toString()
        if (value.isEmpty()) return
        val error = onTextRequested?.invoke(value)
        if (error != null) {
            onInputError?.invoke(error)
            return
        }
        val codePointCount = value.codePointCount(0, value.length)
        composerInput.text.clear()
        closeComposer()
        onInputSent?.invoke(codePointCount)
        onTerminalFocusRequested?.invoke()
    }

    private fun closeComposer() {
        inputMethodManager().hideSoftInputFromWindow(composerInput.windowToken, 0)
        composerInput.clearFocus()
        composerRow.visibility = View.GONE
        keyboardBody.visibility = View.VISIBLE
    }

    private fun updateModifierButtons() {
        modifierButtons.forEach { (modifier, button) ->
            val active = modifier in activeModifiers
            button.backgroundTintList =
                ColorStateList.valueOf(if (active) ACCENT_COLOR else KEY_COLOR)
            button.setTextColor(if (active) Color.rgb(8, 15, 24) else Color.WHITE)
        }
    }

    private fun TerminalAction.androidKeyCode(): Int =
        when (this) {
            TerminalAction.ESCAPE -> KeyEvent.KEYCODE_ESCAPE
            TerminalAction.TAB -> KeyEvent.KEYCODE_TAB
            TerminalAction.ENTER -> KeyEvent.KEYCODE_ENTER
            TerminalAction.BACKSPACE -> KeyEvent.KEYCODE_DEL
            TerminalAction.FORWARD_DELETE -> KeyEvent.KEYCODE_FORWARD_DEL
            TerminalAction.INSERT -> KeyEvent.KEYCODE_INSERT
            TerminalAction.HOME -> KeyEvent.KEYCODE_MOVE_HOME
            TerminalAction.END -> KeyEvent.KEYCODE_MOVE_END
            TerminalAction.PAGE_UP -> KeyEvent.KEYCODE_PAGE_UP
            TerminalAction.PAGE_DOWN -> KeyEvent.KEYCODE_PAGE_DOWN
            TerminalAction.ARROW_LEFT -> KeyEvent.KEYCODE_DPAD_LEFT
            TerminalAction.ARROW_DOWN -> KeyEvent.KEYCODE_DPAD_DOWN
            TerminalAction.ARROW_UP -> KeyEvent.KEYCODE_DPAD_UP
            TerminalAction.ARROW_RIGHT -> KeyEvent.KEYCODE_DPAD_RIGHT
            TerminalAction.F1 -> KeyEvent.KEYCODE_F1
            TerminalAction.F2 -> KeyEvent.KEYCODE_F2
            TerminalAction.F3 -> KeyEvent.KEYCODE_F3
            TerminalAction.F4 -> KeyEvent.KEYCODE_F4
            TerminalAction.F5 -> KeyEvent.KEYCODE_F5
            TerminalAction.F6 -> KeyEvent.KEYCODE_F6
            TerminalAction.F7 -> KeyEvent.KEYCODE_F7
            TerminalAction.F8 -> KeyEvent.KEYCODE_F8
            TerminalAction.F9 -> KeyEvent.KEYCODE_F9
            TerminalAction.F10 -> KeyEvent.KEYCODE_F10
            TerminalAction.F11 -> KeyEvent.KEYCODE_F11
            TerminalAction.F12 -> KeyEvent.KEYCODE_F12
        }

    private fun inputMethodManager(): InputMethodManager =
        context.getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager

    private fun roundedBackground(color: Int, radiusDp: Int, strokeColor: Int) =
        GradientDrawable().apply {
            setColor(color)
            cornerRadius = dp(radiusDp).toFloat()
            setStroke(dp(1), strokeColor)
        }

    private fun LayoutParams.withMargins(left: Int, top: Int, right: Int, bottom: Int) =
        apply { setMargins(dp(left), dp(top), dp(right), dp(bottom)) }

    private fun dp(value: Int): Int =
        (value * resources.displayMetrics.density).toInt()

    private companion object {
        const val KEY_HEIGHT_DP = 42
        const val SPECIAL_COLUMN_COUNT = 6
        const val SPECIAL_KEY_WIDTH_DP = 54
        const val KEYBOARD_CONTENT_MIN_WIDTH_DP = 1_070
        val PANEL_COLOR: Int = Color.argb(244, 18, 23, 32)
        val BORDER_COLOR: Int = Color.rgb(64, 74, 92)
        val KEY_COLOR: Int = Color.rgb(45, 54, 69)
        val ACCENT_COLOR: Int = Color.rgb(102, 217, 239)
        val TEXT_MUTED: Int = Color.rgb(180, 190, 205)
    }
}
