package com.example.wezterm_android

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.content.pm.ApplicationInfo
import android.graphics.Color
import android.graphics.Rect
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.text.InputType
import android.view.ActionMode
import android.view.Gravity
import android.view.Menu
import android.view.MenuItem
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.TextView
import androidx.activity.enableEdgeToEdge
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.app.AppCompatActivity
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import com.google.android.material.textfield.TextInputEditText
import com.google.android.material.textfield.TextInputLayout
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import kotlin.math.max

private data class DialogTextField(
    val container: TextInputLayout,
    val input: TextInputEditText,
)

class MainActivity : AppCompatActivity() {
    private lateinit var statusText: TextView
    private lateinit var sshButton: Button
    private lateinit var muxButton: Button
    private lateinit var keyboardButton: Button
    private lateinit var historyBottomButton: Button
    private lateinit var muxToolbar: LinearLayout
    private lateinit var muxTabText: TextView
    private lateinit var muxPreviousButton: Button
    private lateinit var muxNextButton: Button
    private lateinit var muxNewButton: Button
    private lateinit var muxCloseButton: Button
    private lateinit var terminalSurface: TerminalSurfaceView
    private lateinit var terminalKeyboard: TerminalKeyboardView

    private val mainHandler = Handler(Looper.getMainLooper())
    private var rendererDetail = ""
    private var pollPausedForChallenge = false
    private var activeChallengeDialog: AlertDialog? = null
    private var remoteWasReady = false
    private var muxTabCount = 0
    private var muxCommandInFlight = false
    private var rendererReady = false
    private var activityStarted = false
    private var selectionActionMode: ActionMode? = null
    private var muxReconnectAttempt = 0
    private var muxReconnectScheduled = false
    private var muxAutoReconnectStarting = false
    private var lastViewportOffset = 0

    private val retryMuxConnection = Runnable {
        muxReconnectScheduled = false
        maybeAutoReconnectMux()
    }

    private val terminalSelectionCallback =
        object : ActionMode.Callback2() {
            override fun onCreateActionMode(mode: ActionMode, menu: Menu): Boolean {
                menu.add(0, ACTION_COPY, 0, R.string.selection_copy)
                    .setShowAsAction(MenuItem.SHOW_AS_ACTION_ALWAYS)
                menu.add(0, ACTION_PASTE, 1, R.string.selection_paste)
                    .setShowAsAction(MenuItem.SHOW_AS_ACTION_ALWAYS)
                menu.add(0, ACTION_SELECT_ALL, 2, R.string.selection_select_all)
                    .setShowAsAction(MenuItem.SHOW_AS_ACTION_IF_ROOM)
                menu.add(0, ACTION_CANCEL, 3, R.string.selection_cancel)
                    .setShowAsAction(MenuItem.SHOW_AS_ACTION_IF_ROOM)
                return true
            }

            override fun onPrepareActionMode(mode: ActionMode, menu: Menu): Boolean {
                menu.findItem(ACTION_PASTE)?.isEnabled = clipboardText() != null
                return true
            }

            override fun onActionItemClicked(mode: ActionMode, item: MenuItem): Boolean =
                when (item.itemId) {
                    ACTION_COPY -> {
                        copyTerminalSelection()
                        true
                    }
                    ACTION_PASTE -> {
                        pasteClipboardIntoTerminal()
                        true
                    }
                    ACTION_SELECT_ALL -> {
                        if (terminalSurface.selectAllVisible()) mode.invalidateContentRect()
                        true
                    }
                    ACTION_CANCEL -> {
                        mode.finish()
                        true
                    }
                    else -> false
                }

            override fun onDestroyActionMode(mode: ActionMode) {
                if (selectionActionMode === mode) selectionActionMode = null
                terminalSurface.clearSelectionMode()
            }

            override fun onGetContentRect(
                mode: ActionMode,
                view: View,
                outRect: Rect,
            ) {
                terminalSurface.selectionContentRect(outRect)
            }
        }

    private val pollSshEvents =
        object : Runnable {
            override fun run() {
                if (NativeBridge.nativeSshHasSession() && !pollPausedForChallenge) {
                    NativeBridge.nativeSshPumpTerminal()
                    NativeBridge.nativeSshPollEvent()?.let(::handleSshEvent)
                }
                if (NativeBridge.nativeMuxHasSession()) {
                    NativeBridge.nativeMuxPollEvent()?.let(::handleMuxEvent)
                    if (NativeBridge.nativeMuxReady()) {
                        NativeBridge.nativeMuxPumpTerminal()
                    }
                }
                updateTerminalInputState()
                updateConnectionControls()
                mainHandler.postDelayed(this, SSH_POLL_INTERVAL_MS)
            }
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()

        val root = FrameLayout(this).apply {
            setBackgroundColor(Color.BLACK)
        }
        val contentColumn = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
        }
        root.addView(
            contentColumn,
            FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT,
            ),
        )

        // The keyboard is a normal child below this weighted container.  It
        // therefore consumes layout height instead of covering terminal rows.
        // SurfaceView receives surfaceChanged whenever that split moves, and
        // the native side resizes both its WezTerm model and the remote PTY.
        val terminalContainer = FrameLayout(this)
        contentColumn.addView(
            terminalContainer,
            LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                0,
                1f,
            ),
        )
        terminalSurface = TerminalSurfaceView(this)
        terminalContainer.addView(
            terminalSurface,
            FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT,
            ),
        )

        terminalKeyboard = TerminalKeyboardView(this).apply {
            visibility = android.view.View.GONE
            onKeyRequested = { keyCode, unicodeCodePoint, metaState ->
                finishTerminalSelection()
                NativeBridge.nativeKeyEvent(keyCode, unicodeCodePoint, metaState, true)
                terminalSurface.noteInputReturnedToLiveBottom()
            }
            onTextRequested = { text ->
                finishTerminalSelection()
                val error = NativeBridge.nativeSshWriteText(text)
                terminalSurface.noteInputReturnedToLiveBottom()
                error
            }
            onInputError = { error ->
                statusText.text = getString(R.string.keyboard_send_failed, error)
            }
            onInputSent = { codePoints ->
                statusText.text = getString(R.string.keyboard_sent, codePoints)
            }
            onDismissRequested = { hideTerminalKeyboard() }
            onTerminalFocusRequested = { terminalSurface.requestFocus() }
        }
        terminalSurface.onTerminalTapped = {
            if (remoteTerminalReady()) showTerminalKeyboard()
        }
        terminalSurface.onSelectionStarted = { startTerminalSelection() }
        terminalSurface.onSelectionExitRequested = { finishTerminalSelection() }
        terminalSurface.onViewportOffsetChanged = { offset -> updateViewportStatus(offset) }

        val statusBar = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(dp(10), dp(4), dp(6), dp(4))
            setBackgroundColor(Color.argb(210, 24, 29, 39))
        }
        statusText = TextView(this).apply {
            text = getString(R.string.renderer_starting)
            setTextColor(Color.WHITE)
            textSize = 12f
        }
        sshButton = Button(this).apply {
            text = getString(R.string.ssh_connect)
            textSize = 11f
            minHeight = 0
            minWidth = 0
            setPadding(dp(12), dp(2), dp(12), dp(2))
            setOnClickListener {
                if (NativeBridge.nativeSshHasSession()) {
                    showDisconnectConfirmation()
                } else {
                    showConnectionDialog(useMux = false)
                }
            }
        }
        muxButton = Button(this).apply {
            text = getString(R.string.mux_attach)
            contentDescription = getString(R.string.mux_connection_title)
            textSize = 11f
            minHeight = 0
            minWidth = 0
            setPadding(dp(12), dp(2), dp(12), dp(2))
            setOnClickListener {
                if (NativeBridge.nativeMuxHasSession()) {
                    showMuxDetachConfirmation()
                } else if (muxReconnectScheduled || muxAutoReconnectStarting) {
                    disableMuxAutoReconnect()
                    statusText.text = getString(R.string.mux_reconnect_cancelled)
                    updateConnectionControls()
                } else {
                    showConnectionDialog(useMux = true)
                }
            }
        }
        keyboardButton = Button(this).apply {
            text = getString(R.string.keyboard_toggle)
            contentDescription = getString(R.string.keyboard_toggle_description)
            textSize = 11f
            minHeight = 0
            minWidth = 0
            setPadding(dp(12), dp(2), dp(12), dp(2))
            isEnabled = false
            setOnClickListener { terminalKeyboard.togglePanel() }
        }
        val settingsButton = Button(this).apply {
            text = getString(R.string.settings_button)
            contentDescription = getString(R.string.settings_button_description)
            textSize = 14f
            minHeight = 0
            minWidth = 0
            setPadding(dp(10), dp(2), dp(10), dp(2))
            setOnClickListener {
                startActivity(Intent(this@MainActivity, SettingsActivity::class.java))
            }
        }
        historyBottomButton = Button(this).apply {
            text = getString(R.string.history_live_button)
            contentDescription = getString(R.string.history_live_description)
            textSize = 10f
            minHeight = 0
            minWidth = 0
            setPadding(dp(10), dp(2), dp(10), dp(2))
            visibility = View.GONE
            setOnClickListener { terminalSurface.scrollToBottom() }
        }
        statusBar.addView(
            statusText,
            LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f),
        )
        statusBar.addView(
            historyBottomButton,
            LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ),
        )
        statusBar.addView(
            settingsButton,
            LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ),
        )
        statusBar.addView(
            keyboardButton,
            LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ),
        )
        statusBar.addView(
            sshButton,
            LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ),
        )
        statusBar.addView(
            muxButton,
            LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ),
        )
        contentColumn.addView(
            statusBar,
            0,
            LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ),
        )

        muxTabText = TextView(this).apply {
            text = getString(R.string.mux_no_tabs)
            setTextColor(Color.WHITE)
            textSize = 12f
            maxLines = 1
        }
        muxPreviousButton = compactToolbarButton("◀", R.string.mux_previous_tab) {
            runMuxCommand(getString(R.string.mux_switching_tab)) {
                NativeBridge.nativeMuxActivateRelative(-1)
            }
        }
        muxNewButton = compactToolbarButton("+", R.string.mux_new_tab) {
            runMuxCommand(getString(R.string.mux_creating_tab), NativeBridge::nativeMuxSpawnTab)
        }
        muxCloseButton = compactToolbarButton("×", R.string.mux_close_tab) {
            showMuxCloseConfirmation()
        }
        muxNextButton = compactToolbarButton("▶", R.string.mux_next_tab) {
            runMuxCommand(getString(R.string.mux_switching_tab)) {
                NativeBridge.nativeMuxActivateRelative(1)
            }
        }
        muxToolbar = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            visibility = View.GONE
            setPadding(dp(10), dp(2), dp(6), dp(2))
            setBackgroundColor(Color.rgb(17, 22, 31))
            addView(
                muxTabText,
                LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f),
            )
            addView(muxPreviousButton)
            addView(muxNewButton)
            addView(muxCloseButton)
            addView(muxNextButton)
        }
        contentColumn.addView(
            muxToolbar,
            1,
            LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ),
        )
        contentColumn.addView(
            terminalKeyboard,
            LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ),
        )

        terminalSurface.onRendererStatusChanged = { ready, detail ->
            rendererReady = ready
            rendererDetail = detail
            if (!NativeBridge.nativeSshHasSession() && !NativeBridge.nativeMuxHasSession()) {
                statusText.text = if (ready) {
                    getString(R.string.renderer_ready, detail)
                } else {
                    getString(R.string.renderer_failed, detail)
                }
            }
            if (ready) maybeAutoReconnectMux()
        }

        ViewCompat.setOnApplyWindowInsetsListener(root) { view, windowInsets ->
            val safe = windowInsets.getInsets(
                WindowInsetsCompat.Type.systemBars() or
                    WindowInsetsCompat.Type.displayCutout(),
            )
            val ime = windowInsets.getInsets(WindowInsetsCompat.Type.ime())
            view.setPadding(
                safe.left,
                safe.top,
                safe.right,
                max(safe.bottom, ime.bottom),
            )
            windowInsets
        }

        setContentView(root)
        ViewCompat.requestApplyInsets(root)
        updateTerminalInputState()
    }

    override fun onStart() {
        super.onStart()
        activityStarted = true
        mainHandler.removeCallbacks(pollSshEvents)
        if (NativeBridge.nativeSshHasSession() && activeChallengeDialog == null) {
            NativeBridge.nativeSshPendingEvent()?.let(::handleSshEvent)
        }
        if (NativeBridge.nativeMuxReady()) {
            NativeBridge.nativeMuxCurrentTabs()?.let(::restoreMuxTabs)
        }
        updateTerminalInputState()
        updateConnectionControls()
        mainHandler.post(pollSshEvents)
        terminalSurface.post { maybeAutoReconnectMux() }
    }

    override fun onStop() {
        activityStarted = false
        mainHandler.removeCallbacks(retryMuxConnection)
        muxReconnectScheduled = false
        finishTerminalSelection()
        mainHandler.removeCallbacks(pollSshEvents)
        super.onStop()
    }

    override fun onDestroy() {
        activeChallengeDialog?.dismiss()
        activeChallengeDialog = null
        super.onDestroy()
    }

    private fun showConnectionDialog(useMux: Boolean) {
        val preferences = getSharedPreferences(PREFERENCES_NAME, MODE_PRIVATE)
        val debugIdentity = debugIdentityFile()
        val savedUser = preferences.getString(PREFERENCE_USER, "") ?: ""
        val hostInput = connectionInput(
            hint = getString(R.string.ssh_host),
            value = preferences.getString(PREFERENCE_HOST, "") ?: "",
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI,
        )
        val userInput = connectionInput(
            hint = getString(R.string.ssh_user),
            value = savedUser,
            inputType = InputType.TYPE_CLASS_TEXT,
        )
        val portInput = connectionInput(
            hint = getString(R.string.ssh_port),
            value = preferences.getInt(PREFERENCE_PORT, 22).toString(),
            inputType = InputType.TYPE_CLASS_NUMBER,
        )
        val defaultRemoteWezterm = if (savedUser.isBlank()) {
            "wezterm"
        } else {
            "/home/$savedUser/.local/bin/wezterm"
        }
        val remoteWeztermInput = connectionInput(
            hint = getString(R.string.mux_remote_wezterm_path),
            value = preferences.getString(PREFERENCE_REMOTE_WEZTERM, defaultRemoteWezterm)
                ?: defaultRemoteWezterm,
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI,
        )
        val content = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(20), dp(8), dp(20), 0)
            addView(hostInput.container)
            addView(userInput.container)
            addView(portInput.container)
            if (useMux) addView(remoteWeztermInput.container)
        }

        val dialog = MaterialAlertDialogBuilder(this)
            .setTitle(if (useMux) R.string.mux_connection_title else R.string.ssh_connection_title)
            .setMessage(
                if (useMux) {
                    getString(
                        if (debugIdentity == null) {
                            R.string.mux_connection_boundary
                        } else {
                            R.string.mux_connection_boundary_debug_identity
                        },
                    )
                } else if (debugIdentity == null) {
                    getString(R.string.ssh_connection_boundary)
                } else {
                    getString(R.string.ssh_connection_boundary_debug_identity)
                },
            )
            .setView(content)
            .setNegativeButton(android.R.string.cancel, null)
            .setPositiveButton(if (useMux) R.string.mux_attach else R.string.ssh_connect, null)
            .create()
        dialog.setOnShowListener {
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener {
                val port = portInput.input.text.toString().toIntOrNull()
                if (port == null || port !in 1..65535) {
                    portInput.container.error = getString(R.string.ssh_invalid_port)
                    return@setOnClickListener
                }
                portInput.container.error = null
                val host = hostInput.input.text.toString().trim()
                val user = userInput.input.text.toString().trim()
                val remoteWeztermPath = remoteWeztermInput.input.text.toString().trim()
                if (useMux) disableMuxAutoReconnect()
                val error = if (useMux) {
                    NativeBridge.nativeMuxStart(
                        host,
                        user,
                        port,
                        filesDir.absolutePath,
                        debugIdentity?.absolutePath.orEmpty(),
                        remoteWeztermPath,
                    )
                } else {
                    NativeBridge.nativeSshStart(
                        host,
                        user,
                        port,
                        filesDir.absolutePath,
                        debugIdentity?.absolutePath.orEmpty(),
                    )
                }
                if (error != null) {
                    statusText.text = getString(
                        if (useMux) R.string.mux_start_failed else R.string.ssh_start_failed,
                        error,
                    )
                    return@setOnClickListener
                }
                preferences.edit()
                    .putString(PREFERENCE_HOST, host)
                    .putString(PREFERENCE_USER, user)
                    .putInt(PREFERENCE_PORT, port)
                    .apply {
                        if (useMux) {
                            putString(PREFERENCE_REMOTE_WEZTERM, remoteWeztermPath)
                            putBoolean(PREFERENCE_MUX_AUTO_REATTACH, false)
                        }
                    }
                    .apply()
                statusText.text = getString(
                    if (useMux) R.string.mux_connecting else R.string.ssh_connecting,
                    user,
                    host,
                    port,
                )
                updateConnectionControls()
                dialog.dismiss()
            }
        }
        dialog.show()
    }

    private fun connectionInput(hint: String, value: String, inputType: Int): DialogTextField {
        val input = TextInputEditText(this).apply {
            setText(value)
            this.inputType = inputType
            isSingleLine = true
            selectAll()
        }
        val container = TextInputLayout(
            this,
            null,
            com.google.android.material.R.attr.textInputOutlinedStyle,
        ).apply {
            this.hint = hint
            boxBackgroundMode = TextInputLayout.BOX_BACKGROUND_OUTLINE
            setPadding(0, dp(4), 0, dp(4))
            addView(
                input,
                LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT,
                    ViewGroup.LayoutParams.WRAP_CONTENT,
                ),
            )
        }
        return DialogTextField(container, input)
    }

    private fun showDisconnectConfirmation() {
        MaterialAlertDialogBuilder(this)
            .setTitle(R.string.ssh_disconnect_title)
            .setMessage(R.string.ssh_disconnect_message)
            .setNegativeButton(android.R.string.cancel, null)
            .setPositiveButton(R.string.ssh_disconnect) { _, _ -> disconnectSession() }
            .show()
    }

    private fun disconnectSession() {
        activeChallengeDialog?.dismiss()
        activeChallengeDialog = null
        pollPausedForChallenge = false
        val error = NativeBridge.nativeSshDisconnect()
        statusText.text = if (error == null) {
            getString(R.string.renderer_ready, rendererDetail)
        } else {
            getString(R.string.ssh_disconnect_failed, error)
        }
        updateTerminalInputState()
        updateConnectionControls()
    }

    private fun handleSshEvent(encoded: String) {
        val event = try {
            JSONObject(encoded)
        } catch (error: Exception) {
            statusText.text = getString(R.string.ssh_event_invalid, error.message ?: "JSON")
            return
        }

        when (event.optString("type")) {
            "banner" -> {
                val message = event.optNullableString("message")
                if (!message.isNullOrBlank()) {
                    statusText.text = message
                }
            }
            "verify_host" -> showHostVerification(event.getString("message"))
            "authenticate" -> showAuthentication(event)
            "host_verification_failed" -> {
                val address = event.optString("remote_address")
                val fingerprint = event.optString("fingerprint")
                showFatalSshError(getString(R.string.ssh_host_key_changed, address, fingerprint))
            }
            "authenticated" -> {
                val error = NativeBridge.nativeSshOpenPty()
                if (error == null) {
                    statusText.text = getString(R.string.ssh_opening_pty)
                } else {
                    showFatalSshError(error)
                }
            }
            "pty_ready" -> {
                statusText.text = getString(R.string.ssh_pty_ready)
                terminalSurface.requestFocus()
                NativeBridge.nativeSshPumpTerminal()
                updateTerminalInputState(showWhenReady = true)
            }
            "pty_exited" -> {
                val exitCode = if (event.isNull("exit_code")) "?" else event.getInt("exit_code").toString()
                showFatalSshError(getString(R.string.ssh_pty_exited, exitCode))
            }
            "pty_error" -> showFatalSshError(event.optString("message", getString(R.string.ssh_unknown_error)))
            "error", "control_error" -> {
                showFatalSshError(event.optString("message", getString(R.string.ssh_unknown_error)))
            }
            else -> statusText.text = getString(R.string.ssh_event_invalid, encoded)
        }
    }

    private fun handleMuxEvent(encoded: String) {
        val event = try {
            JSONObject(encoded)
        } catch (error: Exception) {
            statusText.text = getString(R.string.mux_event_invalid, error.message ?: "JSON")
            return
        }

        when (event.optString("type")) {
            "connecting" -> statusText.text = getString(R.string.mux_negotiating)
            "attached" -> {
                muxTabCount = event.optInt("tab_count", 0)
                getSharedPreferences(PREFERENCES_NAME, MODE_PRIVATE)
                    .edit()
                    .putBoolean(PREFERENCE_MUX_AUTO_REATTACH, true)
                    .apply()
                muxReconnectAttempt = 0
                muxReconnectScheduled = false
                muxAutoReconnectStarting = false
                mainHandler.removeCallbacks(retryMuxConnection)
                statusText.text = getString(
                    R.string.mux_attached,
                    muxTabCount,
                    event.optInt("codec_version", 0),
                )
                terminalSurface.requestFocus()
                terminalSurface.scrollToBottom()
                NativeBridge.nativeMuxPumpTerminal()
                updateTerminalInputState(showWhenReady = muxTabCount > 0)
                updateConnectionControls()
            }
            "tabs_changed" -> applyMuxTabs(event.optJSONArray("tabs") ?: JSONArray())
            "detached" -> {
                resetMuxUi()
                if (muxAutoReconnectEnabled()) {
                    scheduleMuxReconnect(getString(R.string.mux_connection_lost))
                } else {
                    statusText.text = getString(R.string.mux_detached)
                }
            }
            "error" -> handleMuxFailure(
                event.optString("message", getString(R.string.ssh_unknown_error)),
            )
            else -> statusText.text = getString(R.string.mux_event_invalid, encoded)
        }
    }

    private fun restoreMuxTabs(encoded: String) {
        try {
            applyMuxTabs(JSONArray(encoded), updateStatus = false)
        } catch (error: Exception) {
            statusText.text = getString(R.string.mux_event_invalid, error.message ?: "JSON")
        }
    }

    private fun applyMuxTabs(tabs: JSONArray, updateStatus: Boolean = true) {
        muxTabCount = tabs.length()
        var activeIndex = -1
        var activeTitle = ""
        for (index in 0 until tabs.length()) {
            val tab = tabs.optJSONObject(index) ?: continue
            if (tab.optBoolean("active", false)) {
                activeIndex = index
                activeTitle = tab.optString("title").replace(Regex("[\\r\\n\\t]"), " ")
                break
            }
        }

        if (muxTabCount == 0) {
            muxTabText.text = getString(R.string.mux_no_tabs)
            if (updateStatus) statusText.text = getString(R.string.mux_no_tabs_status)
        } else {
            val displayIndex = if (activeIndex >= 0) activeIndex + 1 else 1
            val displayTitle = activeTitle.ifBlank { getString(R.string.mux_untitled_tab) }.take(80)
            muxTabText.text = getString(
                R.string.mux_tab_summary,
                displayIndex,
                muxTabCount,
                displayTitle,
            )
            if (updateStatus) {
                statusText.text = getString(
                    R.string.mux_active_tab,
                    displayIndex,
                    muxTabCount,
                    displayTitle,
                )
            }
        }
        updateTerminalInputState()
        updateConnectionControls()
    }

    private fun showMuxCloseConfirmation() {
        if (muxTabCount == 0 || muxCommandInFlight) return
        MaterialAlertDialogBuilder(this)
            .setTitle(R.string.mux_close_tab_title)
            .setMessage(R.string.mux_close_tab_message)
            .setNegativeButton(android.R.string.cancel, null)
            .setPositiveButton(R.string.mux_close_tab) { _, _ ->
                runMuxCommand(
                    getString(R.string.mux_closing_tab),
                    NativeBridge::nativeMuxCloseActiveTab,
                )
            }
            .show()
    }

    private fun showMuxDetachConfirmation() {
        if (!NativeBridge.nativeMuxHasSession() || muxCommandInFlight) return
        MaterialAlertDialogBuilder(this)
            .setTitle(R.string.mux_detach_title)
            .setMessage(R.string.mux_detach_message)
            .setNegativeButton(android.R.string.cancel, null)
            .setPositiveButton(R.string.mux_detach) { _, _ -> detachMuxSession() }
            .show()
    }

    private fun runMuxCommand(progress: String, command: () -> String?) {
        if (!NativeBridge.nativeMuxReady() || muxCommandInFlight) return
        muxCommandInFlight = true
        statusText.text = progress
        updateConnectionControls()
        Thread(
            {
                val error = try {
                    command()
                } catch (failure: Throwable) {
                    failure.message ?: failure.javaClass.simpleName
                }
                mainHandler.post {
                    muxCommandInFlight = false
                    if (error == null) {
                        NativeBridge.nativeMuxPollEvent()?.let(::handleMuxEvent)
                        NativeBridge.nativeMuxPumpTerminal()
                    } else {
                        statusText.text = getString(R.string.mux_command_failed, error)
                    }
                    updateTerminalInputState()
                    updateConnectionControls()
                }
            },
            "wezterm-mux-ui-command",
        ).start()
    }

    private fun detachMuxSession(
        clearAutoReconnect: Boolean = true,
        afterDetach: (() -> Unit)? = null,
    ) {
        if (muxCommandInFlight) return
        if (clearAutoReconnect) disableMuxAutoReconnect()
        muxCommandInFlight = true
        statusText.text = if (clearAutoReconnect) {
            getString(R.string.mux_detaching)
        } else {
            getString(R.string.mux_reconnect_cleaning_up)
        }
        updateConnectionControls()
        Thread(
            {
                val error = try {
                    NativeBridge.nativeMuxDetach()
                } catch (failure: Throwable) {
                    failure.message ?: failure.javaClass.simpleName
                }
                mainHandler.post {
                    muxCommandInFlight = false
                    resetMuxUi()
                    if (afterDetach != null) {
                        afterDetach()
                    } else {
                        statusText.text = if (error == null) {
                            getString(R.string.mux_detached)
                        } else {
                            getString(R.string.mux_detach_failed, error)
                        }
                    }
                    updateTerminalInputState()
                    updateConnectionControls()
                }
            },
            "wezterm-mux-detach",
        ).start()
    }

    private fun handleMuxFailure(message: String) {
        if (!muxAutoReconnectEnabled()) {
            showFatalMuxError(message)
            return
        }
        if (muxCommandInFlight) return
        statusText.text = getString(R.string.mux_reconnect_after_error, message)
        detachMuxSession(clearAutoReconnect = false) {
            scheduleMuxReconnect(message)
        }
    }

    private fun maybeAutoReconnectMux() {
        if (!activityStarted || !rendererReady || muxCommandInFlight || muxAutoReconnectStarting) {
            return
        }
        if (!muxAutoReconnectEnabled()) return
        if (NativeBridge.nativeSshHasSession() || NativeBridge.nativeMuxHasSession()) return

        val preferences = getSharedPreferences(PREFERENCES_NAME, MODE_PRIVATE)
        val host = preferences.getString(PREFERENCE_HOST, "")?.trim().orEmpty()
        val user = preferences.getString(PREFERENCE_USER, "")?.trim().orEmpty()
        val port = preferences.getInt(PREFERENCE_PORT, 22)
        val remoteWeztermPath =
            preferences.getString(PREFERENCE_REMOTE_WEZTERM, "wezterm")
                ?.trim()
                .orEmpty()
        if (host.isBlank() || user.isBlank() || port !in 1..65535 || remoteWeztermPath.isBlank()) {
            disableMuxAutoReconnect()
            statusText.text = getString(R.string.mux_reconnect_missing_endpoint)
            return
        }

        muxAutoReconnectStarting = true
        muxReconnectScheduled = false
        statusText.text = getString(R.string.mux_reconnecting, user, host, port)
        updateConnectionControls()
        val error = try {
            NativeBridge.nativeMuxStart(
                host,
                user,
                port,
                filesDir.absolutePath,
                debugIdentityFile()?.absolutePath.orEmpty(),
                remoteWeztermPath,
            )
        } catch (failure: Throwable) {
            failure.message ?: failure.javaClass.simpleName
        }
        muxAutoReconnectStarting = false
        if (error == null) {
            statusText.text = getString(R.string.mux_connecting, user, host, port)
        } else {
            scheduleMuxReconnect(error)
        }
        updateConnectionControls()
    }

    private fun scheduleMuxReconnect(reason: String) {
        if (!activityStarted || !muxAutoReconnectEnabled() || muxReconnectScheduled) return
        if (NativeBridge.nativeSshHasSession() || NativeBridge.nativeMuxHasSession()) return
        val delaysMs = longArrayOf(1_000L, 2_000L, 4_000L, 8_000L, 15_000L, 30_000L)
        val delayMs = delaysMs[muxReconnectAttempt.coerceAtMost(delaysMs.lastIndex)]
        muxReconnectAttempt += 1
        muxReconnectScheduled = true
        statusText.text = getString(
            R.string.mux_reconnect_scheduled,
            reason.take(120),
            (delayMs / 1_000L).coerceAtLeast(1L),
        )
        mainHandler.removeCallbacks(retryMuxConnection)
        mainHandler.postDelayed(retryMuxConnection, delayMs)
        updateConnectionControls()
    }

    private fun muxAutoReconnectEnabled(): Boolean =
        getSharedPreferences(PREFERENCES_NAME, MODE_PRIVATE)
            .getBoolean(PREFERENCE_MUX_AUTO_REATTACH, false)

    private fun disableMuxAutoReconnect() {
        mainHandler.removeCallbacks(retryMuxConnection)
        muxReconnectScheduled = false
        muxReconnectAttempt = 0
        getSharedPreferences(PREFERENCES_NAME, MODE_PRIVATE)
            .edit()
            .putBoolean(PREFERENCE_MUX_AUTO_REATTACH, false)
            .apply()
    }

    private fun showFatalMuxError(message: String) {
        statusText.text = getString(R.string.mux_failed, message)
        if (activeChallengeDialog != null) return
        val dialog = MaterialAlertDialogBuilder(this)
            .setTitle(R.string.mux_error_title)
            .setMessage(message)
            .setPositiveButton(android.R.string.ok) { _, _ -> detachMuxSession() }
            .setCancelable(false)
            .create()
        dialog.setOnDismissListener { activeChallengeDialog = null }
        activeChallengeDialog = dialog
        dialog.show()
    }

    private fun resetMuxUi() {
        muxTabCount = 0
        muxTabText.text = getString(R.string.mux_no_tabs)
        muxToolbar.visibility = View.GONE
        terminalSurface.scrollToBottom()
    }

    private fun startTerminalSelection() {
        selectionActionMode?.finish()
        selectionActionMode = terminalSurface.startActionMode(
            terminalSelectionCallback,
            ActionMode.TYPE_FLOATING,
        )
        if (selectionActionMode == null) {
            terminalSurface.clearSelectionMode()
        } else {
            statusText.text = getString(R.string.selection_mode_active)
        }
    }

    private fun finishTerminalSelection() {
        val activeMode = selectionActionMode
        if (activeMode == null) {
            terminalSurface.clearSelectionMode()
        } else {
            activeMode.finish()
        }
    }

    private fun clipboardText(): String? {
        return try {
            val clipboard = getSystemService(ClipboardManager::class.java)
            val clip = clipboard.primaryClip
            if (clip == null || clip.itemCount == 0) {
                null
            } else {
                clip.getItemAt(0).coerceToText(this)?.toString()?.takeIf(String::isNotEmpty)
            }
        } catch (_: SecurityException) {
            null
        }
    }

    private fun copyTerminalSelection() {
        val selected = NativeBridge.nativeSelectionText()
        if (selected.isNullOrEmpty()) {
            statusText.text = getString(R.string.selection_empty)
            return
        }
        val clipboard = getSystemService(ClipboardManager::class.java)
        clipboard.setPrimaryClip(
            ClipData.newPlainText(getString(R.string.selection_clipboard_label), selected),
        )
        statusText.text = getString(R.string.selection_copied, selected.codePointCount(0, selected.length))
        selectionActionMode?.finish()
    }

    private fun pasteClipboardIntoTerminal() {
        val text = clipboardText()
        if (text == null) {
            statusText.text = getString(R.string.selection_clipboard_empty)
            return
        }
        val error = NativeBridge.nativeSshWriteText(text)
        statusText.text = if (error == null) {
            getString(R.string.selection_pasted, text.codePointCount(0, text.length))
        } else {
            getString(R.string.selection_paste_failed, error)
        }
        selectionActionMode?.finish()
        terminalSurface.noteInputReturnedToLiveBottom()
    }

    private fun updateViewportStatus(offset: Int) {
        val clampedOffset = offset.coerceAtLeast(0)
        val wasInHistory = lastViewportOffset > 0
        lastViewportOffset = clampedOffset
        historyBottomButton.visibility = if (clampedOffset > 0) View.VISIBLE else View.GONE
        if (clampedOffset > 0) {
            statusText.text = getString(R.string.history_offset, clampedOffset)
        } else if (wasInHistory) {
            statusText.text = getString(R.string.history_live_bottom)
        }
    }

    private fun showHostVerification(message: String) {
        if (activeChallengeDialog != null) return
        pollPausedForChallenge = true
        val dialog = MaterialAlertDialogBuilder(this)
            .setTitle(R.string.ssh_host_verification_title)
            .setMessage(message)
            .setCancelable(false)
            .setNegativeButton(R.string.ssh_reject_host) { _, _ ->
                answerHostVerification(false)
            }
            .setPositiveButton(R.string.ssh_trust_host) { _, _ ->
                answerHostVerification(true)
            }
            .create()
        dialog.setOnDismissListener {
            activeChallengeDialog = null
        }
        activeChallengeDialog = dialog
        dialog.show()
    }

    private fun answerHostVerification(trust: Boolean) {
        val error = NativeBridge.nativeSshAnswerHostVerification(trust)
        pollPausedForChallenge = false
        if (error != null) {
            showFatalSshError(error)
        } else {
            statusText.text = if (trust) {
                getString(R.string.ssh_host_trusted)
            } else {
                getString(R.string.ssh_host_rejected)
            }
        }
    }

    private fun showAuthentication(event: JSONObject) {
        if (activeChallengeDialog != null) return
        val prompts = event.getJSONArray("prompts")
        val inputs = mutableListOf<DialogTextField>()
        val content = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(20), dp(8), dp(20), 0)
        }
        event.optNullableString("instructions")
            ?.takeIf(String::isNotBlank)
            ?.let { instructions ->
                content.addView(TextView(this).apply { text = instructions })
            }
        for (index in 0 until prompts.length()) {
            val prompt = prompts.getJSONObject(index)
            val echo = prompt.optBoolean("echo", false)
            val input = connectionInput(
                hint = prompt.optString("text", getString(R.string.ssh_authentication_answer)),
                value = "",
                inputType = if (echo) {
                    InputType.TYPE_CLASS_TEXT
                } else {
                    InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
                },
            )
            inputs += input
            content.addView(input.container)
        }

        pollPausedForChallenge = true
        val username = event.optString("username")
        val title = if (username.isBlank()) {
            getString(R.string.ssh_authentication_title)
        } else {
            getString(R.string.ssh_authentication_title_user, username)
        }
        val dialog = MaterialAlertDialogBuilder(this)
            .setTitle(title)
            .setView(content)
            .setCancelable(false)
            .setNegativeButton(android.R.string.cancel) { _, _ -> disconnectSession() }
            .setPositiveButton(R.string.ssh_submit, null)
            .create()
        dialog.setOnShowListener {
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener {
                val answers = JSONArray()
                inputs.forEach { field -> answers.put(field.input.text.toString()) }
                val error = NativeBridge.nativeSshAnswerAuthentication(answers.toString())
                inputs.forEach { field -> field.input.text?.clear() }
                if (error != null) {
                    statusText.text = getString(R.string.ssh_authentication_failed, error)
                    return@setOnClickListener
                }
                pollPausedForChallenge = false
                dialog.dismiss()
                statusText.text = getString(R.string.ssh_authentication_sent)
            }
        }
        dialog.setOnDismissListener {
            activeChallengeDialog = null
        }
        activeChallengeDialog = dialog
        dialog.show()
        inputs.firstOrNull()?.input?.requestFocus()
    }

    private fun showFatalSshError(message: String) {
        if (activeChallengeDialog != null) return
        pollPausedForChallenge = true
        statusText.text = getString(R.string.ssh_failed, message)
        val dialog = MaterialAlertDialogBuilder(this)
            .setTitle(R.string.ssh_error_title)
            .setMessage(message)
            .setPositiveButton(android.R.string.ok) { _, _ -> disconnectSession() }
            .setCancelable(false)
            .create()
        dialog.setOnDismissListener { activeChallengeDialog = null }
        activeChallengeDialog = dialog
        dialog.show()
    }

    private fun updateConnectionControls() {
        val sshActive = NativeBridge.nativeSshHasSession()
        val muxActive = NativeBridge.nativeMuxHasSession()
        val muxReady = NativeBridge.nativeMuxReady()

        sshButton.text = if (sshActive) {
            getString(R.string.ssh_disconnect)
        } else {
            getString(R.string.ssh_connect)
        }
        sshButton.isEnabled = !muxActive && !muxCommandInFlight
        sshButton.alpha = if (sshButton.isEnabled) 1f else 0.45f

        muxButton.text = when {
            muxActive -> getString(R.string.mux_detach)
            muxReconnectScheduled || muxAutoReconnectStarting ->
                getString(R.string.mux_stop_reconnect)
            else -> getString(R.string.mux_attach)
        }
        muxButton.contentDescription = when {
            muxActive -> getString(R.string.mux_detach_description)
            muxReconnectScheduled || muxAutoReconnectStarting ->
                getString(R.string.mux_stop_reconnect)
            else -> getString(R.string.mux_connection_title)
        }
        muxButton.isEnabled = !sshActive && !muxCommandInFlight
        muxButton.alpha = if (muxButton.isEnabled) 1f else 0.45f

        muxToolbar.visibility = if (muxReady) View.VISIBLE else View.GONE
        val canSwitch = muxReady && muxTabCount > 1 && !muxCommandInFlight
        muxPreviousButton.isEnabled = canSwitch
        muxNextButton.isEnabled = canSwitch
        muxNewButton.isEnabled = muxReady && !muxCommandInFlight
        muxCloseButton.isEnabled = muxReady && muxTabCount > 0 && !muxCommandInFlight
        listOf(
            muxPreviousButton,
            muxNextButton,
            muxNewButton,
            muxCloseButton,
        ).forEach { button -> button.alpha = if (button.isEnabled) 1f else 0.45f }
    }

    private fun updateTerminalInputState(showWhenReady: Boolean = false) {
        val ready = remoteTerminalReady()
        terminalKeyboard.setTerminalReady(ready)
        keyboardButton.isEnabled = ready
        keyboardButton.alpha = if (ready) 1f else 0.45f
        if (ready && (showWhenReady || !remoteWasReady)) {
            showTerminalKeyboard()
        } else if (!ready && terminalKeyboard.isPanelVisible()) {
            hideTerminalKeyboard()
        }
        remoteWasReady = ready
    }

    private fun remoteTerminalReady(): Boolean =
        NativeBridge.nativeSshPtyReady() ||
            (NativeBridge.nativeMuxReady() && muxTabCount > 0)

    private fun showTerminalKeyboard() {
        terminalKeyboard.showPanel()
        terminalSurface.requestFocus()
    }

    private fun hideTerminalKeyboard() {
        terminalKeyboard.hidePanel()
    }

    private fun compactToolbarButton(
        label: String,
        description: Int,
        action: () -> Unit,
    ): Button = Button(this).apply {
        text = label
        contentDescription = getString(description)
        textSize = 11f
        minHeight = 0
        minWidth = 0
        isAllCaps = false
        setPadding(dp(12), dp(1), dp(12), dp(1))
        setOnClickListener { action() }
    }

    private fun JSONObject.optNullableString(name: String): String? =
        if (!has(name) || isNull(name)) null else getString(name)

    private fun debugIdentityFile(): File? {
        if (applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE == 0) return null
        return File(filesDir, DEBUG_IDENTITY_RELATIVE_PATH).takeIf(File::isFile)
    }

    private fun dp(value: Int): Int =
        (value * resources.displayMetrics.density).toInt()

    companion object {
        private const val SSH_POLL_INTERVAL_MS = 100L
        private const val PREFERENCES_NAME = "ssh_endpoint"
        private const val PREFERENCE_HOST = "host"
        private const val PREFERENCE_USER = "user"
        private const val PREFERENCE_PORT = "port"
        private const val PREFERENCE_REMOTE_WEZTERM = "remote_wezterm_path"
        private const val PREFERENCE_MUX_AUTO_REATTACH = "mux_auto_reattach"
        private const val ACTION_COPY = 1_001
        private const val ACTION_PASTE = 1_002
        private const val ACTION_SELECT_ALL = 1_003
        private const val ACTION_CANCEL = 1_004
        private const val DEBUG_IDENTITY_RELATIVE_PATH =
            "ssh/identities/wezterm_android_debug_ed25519"
    }
}
