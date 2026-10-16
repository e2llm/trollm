package com.trollm.app

import android.content.Context

fun flowSetupOverrideStore(context: Context): FlowSetupOverrideStore {
    val preferences = context.applicationContext.getSharedPreferences("flow-setup-overrides-v1", Context.MODE_PRIVATE)
    return FlowSetupOverrideStore(
        read = { key -> preferences.getString(key, null) },
        write = { key, value ->
            val edit = preferences.edit()
            if (value == null) edit.remove(key) else edit.putString(key, value)
            edit.commit()
        },
    )
}
