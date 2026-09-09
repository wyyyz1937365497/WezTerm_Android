package com.example.wezterm_android

import org.junit.Assert.assertEquals
import org.junit.Test

class TerminalComposerDraftsTest {
    @Test
    fun switchingTabsRestoresEachTabsOwnDraft() {
        val drafts = TerminalComposerDrafts()

        assertEquals("", drafts.switchContext("mux:10"))
        drafts.updateActive("第一标签草稿")
        assertEquals("", drafts.switchContext("mux:20"))
        drafts.updateActive("第二标签草稿")
        assertEquals("第一标签草稿", drafts.switchContext("mux:10"))
        assertEquals("第二标签草稿", drafts.switchContext("mux:20"))
    }

    @Test
    fun sentDraftIsNotRestored() {
        val drafts = TerminalComposerDrafts()

        drafts.switchContext("mux:10")
        drafts.updateActive("已发送的草稿")
        drafts.clearActive()
        drafts.switchContext("mux:20")

        assertEquals("", drafts.switchContext("mux:10"))
    }

    @Test
    fun snapshotRestoresDraftsAfterActivityRecreation() {
        val beforeRecreation = TerminalComposerDrafts()
        beforeRecreation.switchContext("mux:10")
        beforeRecreation.updateActive("保留到后台的草稿")
        beforeRecreation.switchContext("mux:20")
        beforeRecreation.updateActive("另一个标签的草稿")
        val saved = beforeRecreation.snapshot()

        val afterRecreation = TerminalComposerDrafts()
        afterRecreation.restore(saved)

        assertEquals("保留到后台的草稿", afterRecreation.switchContext("mux:10"))
        assertEquals("另一个标签的草稿", afterRecreation.switchContext("mux:20"))
    }
}
