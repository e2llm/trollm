package com.trollm.app

import org.json.JSONObject

/** Native admission is produced by the installed runtime, never by the host.
 * Both ordinary UI and private selected start preserve the original raw bytes.
 */
internal object SourceInputAdmission {
    fun attach(association: String, original: String, apkNormalized: String,
               onStage: (String) -> Unit = {}, admit: (String, String) -> String): String {
        onStage("input-association")
        val source = JSONObject(association)
        require(source.getJSONObject("descriptor").getJSONObject("source").sourceString("origin") == "DECLARED")
        onStage("input-native-admission")
        val receipt = JSONObject(admit(original, apkNormalized))
        onStage("input-receipt")
        require(receipt.sourceString("schema") == "trollm.source-input-admission.v1")
        require(receipt.sourceString("apkNormalized") == apkNormalized)
        if (source.has("inputAdmission")) {
            require(JsonValues.same(source.getJSONObject("inputAdmission"), receipt)) {
                "Source input admission conflicts with the installed runtime."
            }
        }
        return source.put("inputAdmission", receipt).toString()
    }
}
