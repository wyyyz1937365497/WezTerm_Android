package com.example.wezterm_android

import android.view.Surface

internal object NativeBridge {
    init {
        System.loadLibrary("wezterm_android")
    }

    @JvmStatic
    external fun nativeSurfaceCreated(
        surface: Surface,
        width: Int,
        height: Int,
        densityDpi: Int,
    ): Boolean

    /** Returns false when the Surface was lost and must be attached again. */
    @JvmStatic
    external fun nativeSurfaceChanged(width: Int, height: Int): Boolean

    @JvmStatic
    external fun nativeSurfaceRedrawNeeded()

    @JvmStatic
    external fun nativeSurfaceDestroyed()

    /** Positive rows move into older history; returns the clamped offset. */
    @JvmStatic
    external fun nativeScrollByRows(deltaRows: Int): Int

    @JvmStatic
    external fun nativeScrollToBottom(): Int

    @JvmStatic
    external fun nativeSelectionStart(x: Float, y: Float): Boolean

    @JvmStatic
    external fun nativeSelectionUpdate(x: Float, y: Float): Boolean

    @JvmStatic
    external fun nativeSelectionSelectAll(): Boolean

    @JvmStatic
    external fun nativeSelectionClear()

    /** Returns null on success or a user-displayable error string. */
    @JvmStatic
    external fun nativeSetTerminalZoom(zoomPercent: Int): String?

    @JvmStatic
    external fun nativeSelectionText(): String?

    /**
     * UI thread only. Captures the current viewport anchor and either queues
     * the SSHMUX export (returns `{"pending":true}`) or exports the local SSH
     * terminal text inline (returns `{"ok":true,"text":...}`). Always returns
     * a JSON envelope; `{"ok":false,"error":...}` carries the failure.
     */
    @JvmStatic
    external fun nativeBeginViewportExport(): String?

    /**
     * Worker thread. Waits for the queued SSHMUX export and returns the same
     * JSON envelope; null means nothing was queued.
     */
    @JvmStatic
    external fun nativeViewportExportWait(): String?

    @JvmStatic
    external fun nativeKeyEvent(
        keyCode: Int,
        unicodeCodePoint: Int,
        metaState: Int,
        isDown: Boolean,
    )

    /** Positive deltas scroll down; SSHMUX forwards a real terminal wheel event. */
    @JvmStatic
    external fun nativeRemoteMouseWheel(x: Float, y: Float, delta: Int): Boolean

    /** Returns null on success or a user-displayable error string. */
    @JvmStatic
    external fun nativeSshStart(
        host: String,
        user: String,
        port: Int,
        appFilesDir: String,
        identityFile: String,
    ): String?

    @JvmStatic
    external fun nativeSshHasSession(): Boolean

    /** Requests one xterm-256color shell PTY without blocking the UI thread. */
    @JvmStatic
    external fun nativeSshOpenPty(): String?

    @JvmStatic
    external fun nativeSshPtyReady(): Boolean

    /** Writes one complete UTF-8 string, used by the dedicated IME composer. */
    @JvmStatic
    external fun nativeSshWriteText(text: String): String?

    /** Drains raw SSH output into wezterm-term and presents a frame if needed. */
    @JvmStatic
    external fun nativeSshPumpTerminal(): Boolean

    /** Returns null on success or a user-displayable error string. */
    @JvmStatic
    external fun nativeSshDisconnect(): String?

    /** Returns one tagged JSON event, or null when no event is ready. */
    @JvmStatic
    external fun nativeSshPollEvent(): String?

    /** Replays an unanswered challenge after Activity recreation. */
    @JvmStatic
    external fun nativeSshPendingEvent(): String?

    /** Returns null on success or a user-displayable error string. */
    @JvmStatic
    external fun nativeSshAnswerHostVerification(trust: Boolean): String?

    /** `answersJson` is a JSON array; secret answers are never logged. */
    @JvmStatic
    external fun nativeSshAnswerAuthentication(answersJson: String): String?

    /** Starts a real WezTerm SSH multiplexing attachment; null means accepted. */
    @JvmStatic
    external fun nativeMuxStart(
        host: String,
        user: String,
        port: Int,
        appFilesDir: String,
        identityFile: String,
        remoteWeztermPath: String,
        preferredRemoteTabId: Long,
    ): String?

    @JvmStatic
    external fun nativeMuxHasSession(): Boolean

    @JvmStatic
    external fun nativeMuxReady(): Boolean

    /** Mirrors the active remote pane into the existing native renderer. */
    @JvmStatic
    external fun nativeMuxPumpTerminal(): Boolean

    /** Returns one tagged JSON attachment/tab event, or null. */
    @JvmStatic
    external fun nativeMuxPollEvent(): String?

    /** JSON array used to restore the toolbar after Activity recreation. */
    @JvmStatic
    external fun nativeMuxCurrentTabs(): String?

    @JvmStatic
    external fun nativeMuxActivateRelative(delta: Int): String?

    /** Selects a stable server-side tab id after foregrounding or reconnecting. */
    @JvmStatic
    external fun nativeMuxActivateTab(remoteTabId: Long): String?

    @JvmStatic
    external fun nativeMuxSpawnTab(): String?

    /** Destructive: terminates every remote pane in the active tab. */
    @JvmStatic
    external fun nativeMuxCloseActiveTab(): String?

    /** Safe: drops only the Android mirror and preserves remote panes. */
    @JvmStatic
    external fun nativeMuxDetach(): String?
}
