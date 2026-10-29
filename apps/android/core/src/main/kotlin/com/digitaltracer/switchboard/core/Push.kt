package com.digitaltracer.switchboard.core

/**
 * What a notification points at. The hub's pushes carry ids only, as FCM data strings; the app
 * looks up the rest.
 */
data class PushTarget(
    val kind: String,
    val sessionId: String? = null,
    val approvalId: String? = null,
    val attentionId: String? = null,
) {
    companion object {
        private val ID = Regex("^[A-Za-z0-9_:.-]{1,64}$")

        /** `null` for anything that isn't one of the hub's pushes, or carries a malformed id. */
        fun parse(data: Map<String, String>): PushTarget? {
            val kind = data["kind"]?.takeIf { it in KINDS } ?: return null
            fun id(key: String): String? {
                val v = data[key] ?: return null
                return if (ID.matches(v)) v else throw IllegalArgumentException(key)
            }
            return runCatching {
                PushTarget(kind, id("session_id"), id("approval_id"), id("attention_id"))
            }.getOrNull()
        }

        private val KINDS = setOf("approval", "finished", "device_paired", "waiting")
    }
}
