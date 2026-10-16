package com.trollm.app

import android.content.Context
import java.io.File

/** IDs remain discoverable independently of installed flow pins and UI lifetime. */
class RunHistory(context: Context) {
    private val preferences = context.applicationContext.getSharedPreferences("run-history", Context.MODE_PRIVATE)
    private val directory = File(context.applicationContext.filesDir, "trollm/runs")

    fun ids(): List<String> = RunIdentity.discover(
        preferences.getStringSet("ids", emptySet()) ?: emptySet(),
        directory.listFiles()?.filter { it.isDirectory }?.map { it.name } ?: emptyList(),
    )

    @Synchronized
    fun record(id: String) {
        require(RunIdentity.valid(id))
        val ids = (preferences.getStringSet("ids", emptySet()) ?: emptySet()).toMutableSet()
        ids.add(id)
        check(preferences.edit().putStringSet("ids", ids).putString("selected", id).commit()) {
            "Unable to persist run identity; recover it from run history"
        }
    }

    fun selected(): String? = preferences.getString("selected", null)?.takeIf(RunIdentity::valid)
    fun select(id: String?) {
        require(id == null || RunIdentity.valid(id))
        preferences.edit().putString("selected", id).apply()
    }
}
