package com.digitaltracer.switchboard.core

import java.io.File
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive

/** `tests/fixtures/app-v1/`, shared with the hub's Rust tests and the iOS package. */
object Fixtures {
    private val root = File(System.getProperty("switchboard.repo"), "tests/fixtures/app-v1")

    fun json(path: String): JsonObject = Json.parseToJsonElement(File(root, path).readText()).jsonObject

    val relayVector: JsonObject by lazy { json("relay-vector.json") }
}

fun JsonObject.str(key: String): String = getValue(key).jsonPrimitive.content

fun JsonObject.obj(key: String): JsonObject = getValue(key).jsonObject

fun hexBytes(s: String): ByteArray = s.chunked(2).map { it.toInt(16).toByte() }.toByteArray()
