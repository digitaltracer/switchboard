package com.digitaltracer.switchboard.core

import com.digitaltracer.switchboard.api.models.Transcript
import kotlinx.serialization.json.Json
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class ConversationTest {
    // Its own decoder: the shared one may only be configured once, by the hub client.
    private fun page(json: String): Transcript = Json.decodeFromString(json)

    @Test
    fun aConversationGrowsJoinsResultsAndGoesBack() {
        var c = Conversation.of(page("""
            {"start":500,"end":900,"items":[
              {"id":"u1","kind":"user","text":"fix it","done":false,"failed":false},
              {"id":"t1","kind":"tool","text":"Ran cargo test","tool":"Bash","done":false,"failed":false}]}
        """))
        assertEquals(listOf("u1", "t1"), c.items.map { it.id })
        assertTrue(c.hasEarlier)

        // The result comes on the next page and fills in its call.
        c = c.append(page("""
            {"start":900,"end":1000,"items":[
              {"id":"t1","kind":"tool","output":"ok","done":true,"failed":false},
              {"id":"a1","kind":"assistant","text":"Fixed.","done":false,"failed":false}]}
        """), askedAfter = 900)
        assertEquals(listOf("u1", "t1", "a1"), c.items.map { it.id })
        assertEquals(Triple("Ran cargo test", "ok", true), Triple(c.items[1].text, c.items[1].output, c.items[1].done))
        assertEquals(1000, c.end)

        // An earlier page's call meets a result that arrived before it.
        c = c.append(page("""
            {"start":1000,"end":1100,"items":[{"id":"t0","kind":"tool","output":"done earlier","done":true,"failed":true}]}
        """), askedAfter = 1000)
        c = c.prepend(page("""
            {"start":0,"end":500,"items":[{"id":"t0","kind":"tool","text":"Ran make","tool":"Bash","done":false,"failed":false}]}
        """))
        assertEquals(listOf("t0", "u1", "t1", "a1"), c.items.map { it.id })
        assertFalse(c.hasEarlier)
        assertEquals("done earlier", c.items[0].output)
        assertTrue(c.items[0].failed)

        // A page that skipped some replaces what's shown.
        c = c.append(page("""
            {"start":5000,"end":5100,"items":[{"id":"a9","kind":"assistant","text":"Later.","done":false,"failed":false}]}
        """), askedAfter = 1100)
        assertEquals(listOf("a9"), c.items.map { it.id })
        assertEquals(5000L, c.start)
    }

    @Test
    fun whatWaitsInTheQueueIsWhatTheLatestPageSays() {
        var c = Conversation.of(page("""
            {"start":0,"end":100,"items":[{"id":"u1","kind":"user","text":"fix it","done":false,"failed":false}],
             "queued":[{"id":"queued:90","kind":"user","text":"also run clippy","done":false,"failed":false}]}
        """))
        assertEquals(listOf("also run clippy"), c.queued.map { it.text })

        // An earlier page says nothing about the queue now.
        c = c.prepend(page("""{"start":0,"end":0,"items":[]}"""))
        assertEquals(1, c.queued.size)

        // Taken in: it's in the conversation and waits no more.
        c = c.append(page("""
            {"start":100,"end":200,"items":[{"id":"q1:0","kind":"user","text":"also run clippy","done":false,"failed":false}]}
        """), askedAfter = 100)
        assertTrue(c.queued.isEmpty())
        assertEquals(listOf("u1", "q1:0"), c.items.map { it.id })
    }
}
