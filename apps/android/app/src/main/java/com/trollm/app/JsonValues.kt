package com.trollm.app

import org.json.JSONArray
import org.json.JSONObject

/** Android platform org.json only. No desktop-only keySet/similar methods.
 * This compares parsed values, never defines a canonical identity or byte hash.
 */
object JsonValues {
    fun keys(value: JSONObject): Set<String> {
        val result = linkedSetOf<String>()
        val names = value.keys()
        while (names.hasNext()) result.add(names.next())
        return result
    }

    fun same(left: Any?, right: Any?): Boolean {
        val leftNull = left == null || left === JSONObject.NULL
        val rightNull = right == null || right === JSONObject.NULL
        if (leftNull || rightNull) return leftNull && rightNull
        return when {
            left is JSONObject && right is JSONObject -> {
                val names = keys(left)
                names == keys(right) && names.all { same(left.get(it), right.get(it)) }
            }
            left is JSONArray && right is JSONArray ->
                left.length() == right.length() && (0 until left.length()).all { same(left.get(it), right.get(it)) }
            left is Number && right is Number -> left.toString() == right.toString()
            left is String && right is String -> left == right
            left is Boolean && right is Boolean -> left == right
            else -> false
        }
    }
}

/** Android getString coerces numbers/booleans; wire string fields must not. */
internal fun JSONObject.sourceString(name: String): String =
    get(name) as? String ?: throw IllegalArgumentException("Source field requires a JSON string: $name")

internal fun JSONArray.sourceString(index: Int): String =
    get(index) as? String ?: throw IllegalArgumentException("Source array element requires a JSON string")
