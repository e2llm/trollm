package com.trollm.app

/** Bounded recent technical text. Full evidence remains in the run journal. */
internal object TechnicalEvents {
    const val MAX_LINES = 256
    const val MAX_BYTES = 256 * 1024

    fun append(current: List<String>, incoming: List<String>): List<String> {
        val recent = ArrayDeque<String>()
        var bytes = 0
        for (line in current + incoming) {
            val size = line.toByteArray(Charsets.UTF_8).size
            if (size > MAX_BYTES) {
                recent.clear()
                bytes = 0
                continue
            }
            recent.addLast(line)
            bytes += size
            while (recent.size > MAX_LINES || bytes > MAX_BYTES) {
                bytes -= recent.removeFirst().toByteArray(Charsets.UTF_8).size
            }
        }
        return recent.toList()
    }
}
