package com.trollm.app

data class ModelOverrideDraft(
    val url: String,
    val sha256: String,
    val sizeBytes: String,
    val ctx: String,
    val temperature: String,
    val topP: String,
) {
    fun applyTo(default: ModelRequirement): ModelRequirement = default.copy(
        url = url, sha256 = sha256,
        sizeBytes = sizeBytes.toLongOrNull() ?: throw IllegalArgumentException("Expected byte size must be a positive whole number."),
        ctx = ctx.toIntOrNull() ?: throw IllegalArgumentException("Context must be a positive whole number."),
        temperature = temperature.toDoubleOrNull() ?: throw IllegalArgumentException("Enter a numeric temperature."),
        topP = topP.toDoubleOrNull() ?: throw IllegalArgumentException("Enter a numeric topP."),
    ).also { it.validate() }

    companion object {
        fun from(model: ModelRequirement) = ModelOverrideDraft(model.url, model.sha256, model.sizeBytes.toString(),
            model.ctx.toString(), model.temperature.toString(), model.topP.toString())
    }
}
