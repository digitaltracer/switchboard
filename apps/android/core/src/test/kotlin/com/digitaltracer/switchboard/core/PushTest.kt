package com.digitaltracer.switchboard.core

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class PushTest {
    @Test
    fun aWaitingPushNamesItsAttention() {
        assertEquals(
            PushTarget("waiting", attentionId = "a1b2"),
            PushTarget.parse(mapOf("kind" to "waiting", "attention_id" to "a1b2")),
        )
        assertEquals(
            PushTarget("approval", sessionId = "s1", approvalId = "ap1"),
            PushTarget.parse(mapOf("kind" to "approval", "session_id" to "s1", "approval_id" to "ap1")),
        )
    }

    @Test
    fun anythingElseIsIgnored() {
        assertNull(PushTarget.parse(emptyMap()))
        assertNull(PushTarget.parse(mapOf("kind" to "marketing")))
        assertNull(PushTarget.parse(mapOf("kind" to "waiting", "attention_id" to "../../etc")))
        assertNull(PushTarget.parse(mapOf("kind" to "waiting", "attention_id" to "x".repeat(65))))
    }
}
