package com.digitaltracer.switchboard.core

import com.digitaltracer.switchboard.api.models.ShipButtonView
import com.digitaltracer.switchboard.api.models.ShipScope
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue
import com.digitaltracer.switchboard.api.models.ShipButtons as ShipButtonsView

class ShipTest {
    @Test
    fun aPullRequestLinkInTheReplyIsFound() {
        val pr = PullRequest.find("Opened [Retry failed imports with backoff](https://github.com/acme/kraft/pull/482) as a draft.")!!
        assertEquals(482, pr.number)
        assertEquals("acme/kraft", pr.repo)
        assertEquals("Retry failed imports with backoff", pr.title)
        assertEquals(7, PullRequest.find("See https://gitlab.com/g/sub/proj/-/merge_requests/7 now")?.number)
        assertEquals(2, PullRequest.find("https://github.com/a/b/pull/1 then https://github.com/a/b/pull/2")?.number)
        assertNull(PullRequest.find("https://github.com/a/b/issues/3"))
    }

    @Test
    fun aShipSkillWithoutAPullRequestSaysSo() {
        val pr = ShipButton("/pr", "Raise PR")
        assertEquals(ShipOutcome.Running, ShipOutcome.of(pr, false, "working"))
        assertEquals(ShipOutcome.NoPullRequest, ShipOutcome.of(pr, true, "The push was denied."))
        assertEquals(ShipOutcome.Done, ShipOutcome.of(ShipButton("/commit", "Commit"), true, "Committed."))
        assertTrue(ShipOutcome.of(pr, true, "https://github.com/a/b/pull/9") is ShipOutcome.Opened)
    }

    @Test
    fun buttonsComeFromTheProjectThenAllThenSuggestions() {
        val v = ShipButtonsView(listOf(ShipButtonView("/pr", "Raise PR")), listOf(ShipScope("kraft", listOf(ShipButtonView("/ship", "Ship")))))
        assertEquals(listOf("/ship"), ShipButtons.effective(v, "kraft").map { it.skill })
        assertEquals(listOf("/pr"), ShipButtons.effective(v, "other").map { it.skill })
        assertEquals(listOf("/pr", "\$fix-ci"), ShipButtons.suggested(listOf("\$fix-ci", "/pr", "/review")).map { it.skill })
        assertEquals("Release note", ShipButtons.label("/plugin:release_note"))
        assertEquals("/pr Draft, base main", ShipButton("/pr", "Raise PR").message(" Draft, base main "))
        assertEquals("\$pr", ShipButton("/pr", "Raise PR").invocation(listOf("\$pr")).skill)
    }
}
