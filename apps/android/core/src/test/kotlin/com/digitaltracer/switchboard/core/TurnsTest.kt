package com.digitaltracer.switchboard.core

import com.digitaltracer.switchboard.api.models.Skills
import com.digitaltracer.switchboard.api.models.Transcript
import kotlinx.serialization.json.Json
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class TurnsTest {
    // Its own decoder: the shared one may only be configured once, by the hub client.
    private val json = Json { ignoreUnknownKeys = true }

    private fun items(list: String) = json.decodeFromString<Transcript>("""{"start":0,"end":1,"items":$list}""").items

    @Test
    fun aReplyWithMoreWorkAfterItIsProgressAndTheTurnsLastReplyIsTheAnswer() {
        val list = items(
            """
            [{"id":"u1","kind":"user","text":"review it","done":false,"failed":false},
             {"id":"t1","kind":"tool","tool":"Read","text":"Read /r/a.py","done":true,"failed":false},
             {"id":"a1","kind":"assistant","text":"Still reviewing.","done":false,"failed":false},
             {"id":"t2","kind":"tool","tool":"Bash","text":"Ran pytest","done":true,"failed":false},
             {"id":"a2","kind":"assistant","text":"Tests pass.","done":false,"failed":false},
             {"id":"a3","kind":"assistant","text":"Round 2: no defects.","done":false,"failed":false},
             {"id":"u2","kind":"user","text":"push","done":false,"failed":false},
             {"id":"a4","kind":"assistant","text":"Pushing.","done":false,"failed":false},
             {"id":"t3","kind":"tool","tool":"Bash","text":"Ran git push","done":false,"failed":false}]
            """,
        )
        val kinds = Turns.entries(list).map { it::class.simpleName to it.id }
        assertEquals(
            listOf(
                "User" to "u1", "Work" to "work:t1", "Progress" to "a1", "Work" to "work:t2", "Progress" to "a2",
                "Reply" to "a3", "User" to "u2", "Progress" to "a4", "Work" to "work:t3",
            ),
            kinds,
        )
    }

    @Test
    fun stepsBetweenMessagesFoldIntoWorkThatSaysWhatCameOfIt() {
        val list = items(
            """
            [{"id":"u1","kind":"user","text":"add retries, then open a PR","done":false,"failed":false,"at":"2026-10-28T13:12:00Z"},
             {"id":"t1","kind":"tool","tool":"Read","text":"Read /r/slack.py","done":true,"failed":false,"at":"2026-10-28T13:12:05Z"},
             {"id":"t2","kind":"tool","tool":"Edit","text":"Edited /r/slack.py","done":true,"failed":false,"added":40,"removed":6,"at":"2026-10-28T13:14:00Z"},
             {"id":"t3","kind":"tool","tool":"Edit","text":"Edited /r/slack.py","done":true,"failed":false,"added":8,"at":"2026-10-28T13:15:00Z"},
             {"id":"t4","kind":"tool","tool":"Bash","text":"Ran pytest -q","done":true,"failed":false,"at":"2026-10-28T13:16:00Z"},
             {"id":"t5","kind":"tool","tool":"Bash","text":"Ran gh pr create","output":"https://github.com/acme/lib/pull/49","done":true,"failed":false,"at":"2026-10-28T13:18:05Z"},
             {"id":"a1","kind":"assistant","text":"PR #49 is open.","done":false,"failed":false},
             {"id":"u2","kind":"user","text":"push both","done":false,"failed":false},
             {"id":"t6","kind":"tool","tool":"Bash","text":"Ran git push","done":true,"failed":false},
             {"id":"a2","kind":"assistant","text":"Pushed.","done":false,"failed":false}]
            """,
        )
        val entries = Turns.entries(list)
        assertEquals(listOf("u1", "work:t1", "a1", "u2", "work:t6", "a2"), entries.map { it.id })
        val work = entries[1] as Turns.Entry.Work
        assertEquals(360L, work.seconds)
        assertEquals(listOf("1 file", "Tests passed", "PR #49"), work.tags.map { it.text })
        assertEquals(48 to 6, work.tags[0].added to work.tags[0].removed)

        val latest = Turns.latest(list)
        assertEquals("Pushed.", latest.reply?.text)
        assertEquals(listOf("Pushed"), latest.tags.map { it.text })
    }

    @Test
    fun skillsFilterByNameFirstThenDescription() {
        val skills = json.decodeFromString<Skills>(
            """{"skills":[{"name":"/compact","description":"Summarise","source":"builtin"},
                          {"name":"/council","description":"Review a plan","source":"user"},
                          {"name":"/ship","description":"Commit, push and open a PR","source":"user"},
                          {"name":"/commit","description":"Atomic commits","source":"user"}]}""",
        ).skills
        assertEquals(listOf("/compact", "/council", "/commit", "/ship"), SkillSearch.filter(skills, "/co").map { it.name })
        assertTrue(SkillSearch.isTyping("/co") && !SkillSearch.isTyping("/commit fix it") && !SkillSearch.isTyping("hi"))
    }
}
