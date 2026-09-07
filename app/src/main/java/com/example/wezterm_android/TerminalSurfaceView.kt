package com.example.wezterm_android

import android.content.Context
import android.graphics.Rect
import android.graphics.PixelFormat
import android.util.Log
import android.view.GestureDetector
import android.view.HapticFeedbackConstants
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.SurfaceHolder
import android.view.SurfaceView
import kotlin.math.abs

/**
 * Android owns this view and its Surface lifecycle. Rust owns only the renderer
 * attached to the current Surface; destroying a Surface must never imply that a
 * future terminal session is destroyed.
 */
internal class TerminalSurfaceView(context: Context) : SurfaceView(context), SurfaceHolder.Callback2 {
    var onRendererStatusChanged: ((Boolean, String) -> Unit)? = null
    var onTerminalTapped: (() -> Unit)? = null
    var onSelectionStarted: (() -> Unit)? = null
    var onSelectionExitRequested: (() -> Unit)? = null
    var onViewportOffsetChanged: ((Int) -> Unit)? = null

    private var nativeSurfaceAttached = false
    private var selectionActive = false
    private var scrollRemainderPx = 0f
    private var viewportOffset = 0
    private var selectionX = 0f
    private var selectionY = 0f
    private val terminalCellHeightPx: Float
        get() = 21f * resources.displayMetrics.density

    private val gestureDetector =
        GestureDetector(
            context,
            object : GestureDetector.SimpleOnGestureListener() {
                override fun onDown(event: MotionEvent): Boolean {
                    requestFocus()
                    scrollRemainderPx = 0f
                    return true
                }

                override fun onSingleTapUp(event: MotionEvent): Boolean {
                    if (!selectionActive) onTerminalTapped?.invoke()
                    return true
                }

                override fun onLongPress(event: MotionEvent) {
                    if (!nativeSurfaceAttached) return
                    if (NativeBridge.nativeSelectionStart(event.x, event.y)) {
                        selectionActive = true
                        selectionX = event.x
                        selectionY = event.y
                        performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
                        onSelectionStarted?.invoke()
                    }
                }

                override fun onScroll(
                    first: MotionEvent?,
                    current: MotionEvent,
                    distanceX: Float,
                    distanceY: Float,
                ): Boolean {
                    if (selectionActive || !nativeSurfaceAttached) return true
                    // Direct manipulation: dragging content down reveals older
                    // rows; dragging it up returns towards the live bottom.
                    scrollRemainderPx += -distanceY
                    dispatchAccumulatedScroll()
                    return true
                }

                override fun onFling(
                    first: MotionEvent?,
                    current: MotionEvent,
                    velocityX: Float,
                    velocityY: Float,
                ): Boolean {
                    if (selectionActive || !nativeSurfaceAttached) return true
                    val rows =
                        (velocityY / (terminalCellHeightPx * 7f))
                            .toInt()
                            .coerceIn(-30, 30)
                    if (rows != 0) dispatchScrollRows(rows)
                    return true
                }
            },
        )

    init {
        holder.setFormat(PixelFormat.OPAQUE)
        holder.addCallback(this)
        isFocusable = true
        isFocusableInTouchMode = true
        contentDescription = context.getString(R.string.terminal_surface_description)
    }

    override fun surfaceCreated(holder: SurfaceHolder) {
        val frame = holder.surfaceFrame
        Log.d(TAG, "surfaceCreated ${frame.width()}x${frame.height()} attached=$nativeSurfaceAttached")
        attachNativeSurface(
            holder,
            frame.width().coerceAtLeast(1),
            frame.height().coerceAtLeast(1),
        )
    }

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
        Log.d(TAG, "surfaceChanged ${width}x${height} attached=$nativeSurfaceAttached")
        if (!nativeSurfaceAttached) {
            attachNativeSurface(holder, width, height)
        } else if (width > 0 && height > 0) {
            nativeSurfaceAttached = NativeBridge.nativeSurfaceChanged(width, height)
            if (!nativeSurfaceAttached) {
                onRendererStatusChanged?.invoke(false, "Surface resize failed; retrying attach")
                // A failed native resize drops its renderer. Retry after this
                // SurfaceView callback so the producer hand-off has settled.
                post { attachNativeSurface(holder, width, height) }
            }
        }
    }

    override fun surfaceRedrawNeeded(holder: SurfaceHolder) {
        if (nativeSurfaceAttached) {
            NativeBridge.nativeSurfaceRedrawNeeded()
        }
    }

    override fun surfaceDestroyed(holder: SurfaceHolder) {
        Log.d(TAG, "surfaceDestroyed attached=$nativeSurfaceAttached")
        // This JNI call is intentionally idempotent. Do not gate it on the
        // Kotlin flag: a prior remote-resize error used to desynchronize that
        // flag from an actually live native renderer.
        NativeBridge.nativeSurfaceDestroyed()
        nativeSurfaceAttached = false
        onRendererStatusChanged?.invoke(false, "Android Surface unavailable")
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        gestureDetector.onTouchEvent(event)
        if (selectionActive && event.actionMasked == MotionEvent.ACTION_MOVE) {
            selectionX = event.x.coerceIn(0f, width.toFloat().coerceAtLeast(1f) - 1f)
            selectionY = event.y.coerceIn(0f, height.toFloat().coerceAtLeast(1f) - 1f)
            NativeBridge.nativeSelectionUpdate(selectionX, selectionY)
        }
        return true
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        if (selectionActive) onSelectionExitRequested?.invoke()
        NativeBridge.nativeKeyEvent(keyCode, event.unicodeChar, event.metaState, true)
        noteInputReturnedToLiveBottom()
        return true
    }

    override fun onKeyUp(keyCode: Int, event: KeyEvent): Boolean {
        NativeBridge.nativeKeyEvent(keyCode, event.unicodeChar, event.metaState, false)
        return true
    }

    fun clearSelectionMode() {
        if (!selectionActive) return
        selectionActive = false
        NativeBridge.nativeSelectionClear()
    }

    fun selectAllVisible(): Boolean =
        NativeBridge.nativeSelectionSelectAll().also { selected ->
            if (selected) {
                selectionActive = true
                selectionX = width / 2f
                selectionY = height / 2f
            }
        }

    fun selectionContentRect(outRect: Rect) {
        val radius = (32f * resources.displayMetrics.density).toInt().coerceAtLeast(1)
        val centerX = selectionX.toInt().coerceIn(0, width.coerceAtLeast(1) - 1)
        val centerY = selectionY.toInt().coerceIn(0, height.coerceAtLeast(1) - 1)
        outRect.set(
            (centerX - radius).coerceAtLeast(0),
            (centerY - radius).coerceAtLeast(0),
            (centerX + radius).coerceAtMost(width),
            (centerY + radius).coerceAtMost(height),
        )
    }

    fun scrollToBottom() {
        val offset = NativeBridge.nativeScrollToBottom()
        if (offset >= 0) notifyViewportOffset(offset)
    }

    fun noteInputReturnedToLiveBottom() {
        if (viewportOffset == 0) return
        viewportOffset = 0
        onViewportOffsetChanged?.invoke(0)
    }

    private fun dispatchAccumulatedScroll() {
        val cellHeight = terminalCellHeightPx.coerceAtLeast(1f)
        if (abs(scrollRemainderPx) < cellHeight) return
        val rows = (scrollRemainderPx / cellHeight).toInt()
        scrollRemainderPx -= rows * cellHeight
        dispatchScrollRows(rows)
    }

    private fun dispatchScrollRows(rows: Int) {
        val offset = NativeBridge.nativeScrollByRows(rows)
        if (offset >= 0) notifyViewportOffset(offset)
    }

    private fun notifyViewportOffset(offset: Int) {
        viewportOffset = offset.coerceAtLeast(0)
        onViewportOffsetChanged?.invoke(viewportOffset)
    }

    private fun attachNativeSurface(holder: SurfaceHolder, width: Int, height: Int) {
        if (!holder.surface.isValid || width <= 0 || height <= 0) {
            return
        }
        nativeSurfaceAttached = NativeBridge.nativeSurfaceCreated(
            holder.surface,
            width,
            height,
            resources.displayMetrics.densityDpi,
        )
        onRendererStatusChanged?.invoke(
            nativeSurfaceAttached,
            if (nativeSurfaceAttached) "WezTerm + HarfBuzz/FreeType / wgpu Vulkan" else "see WezTermAndroid logcat",
        )
    }

    private companion object {
        const val TAG = "WezTermSurface"
    }

}
