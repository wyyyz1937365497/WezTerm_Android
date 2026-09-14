package com.example.wezterm_android

import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.view.View

/**
 * Black terminal-area preview for the zoom slider. White one-pixel lines mark
 * the exact boundaries of every character cell the renderer would use, so
 * dragging the slider shows rows and columns shrinking or growing.
 */
internal class TerminalGridPreviewView(context: Context) : View(context) {
    /** Reports the whole-cell grid that fits the current bounds and zoom. */
    var onGridChanged: ((columns: Int, rows: Int) -> Unit)? = null

    var zoomPercent: Int = TerminalZoom.DEFAULT_PERCENT
        set(value) {
            val clamped = TerminalZoom.clamp(value)
            if (field == clamped) return
            field = clamped
            notifyGridChanged()
            invalidate()
        }

    private val gridPaint =
        Paint(Paint.ANTI_ALIAS_FLAG).apply {
            color = Color.WHITE
            style = Paint.Style.STROKE
            strokeWidth = 1f
        }

    fun gridDimensions(): IntArray {
        val (cellWidth, cellHeight) =
            TerminalZoom.cellSizePx(resources.configuration.densityDpi, zoomPercent)
        return TerminalZoom.gridDimensions(
            width.toFloat(),
            height.toFloat(),
            cellWidth,
            cellHeight,
        )
    }

    override fun onSizeChanged(width: Int, height: Int, oldWidth: Int, oldHeight: Int) {
        super.onSizeChanged(width, height, oldWidth, oldHeight)
        notifyGridChanged()
    }

    override fun onDraw(canvas: Canvas) {
        canvas.drawColor(Color.BLACK)
        val (cellWidth, cellHeight) =
            TerminalZoom.cellSizePx(resources.configuration.densityDpi, zoomPercent)
        val (columns, rows) = gridDimensions()
        for (column in 1 until columns) {
            val x = column * cellWidth
            canvas.drawLine(x, 0f, x, height.toFloat(), gridPaint)
        }
        for (row in 1 until rows) {
            val y = row * cellHeight
            canvas.drawLine(0f, y, width.toFloat(), y, gridPaint)
        }
    }

    private fun notifyGridChanged() {
        val (columns, rows) = gridDimensions()
        onGridChanged?.invoke(columns, rows)
    }
}
