package dk.maarse.app

/**
 * Mentions: "@name" (or "@[name]") in a message. The same rule is used by the web client.
 * A mention is matched against our own name, case-insensitive, and must be a whole word:
 * "@dennis tlf" mentions Dennis, "@dennisk" does not.
 */
object Mentions {
    fun mentions(text: String, name: String?): Boolean {
        val n = name?.trim().orEmpty()
        if (n.isEmpty() || !text.contains('@')) return false
        val re = Regex("(?<![\\p{L}\\p{N}_])@\\[?" + Regex.escape(n) + "\\]?(?![\\p{L}\\p{N}_])",
            setOf(RegexOption.IGNORE_CASE))
        return re.containsMatchIn(text)
    }

    /** What to type to mention [name]: "@name " (a name with a space or a bracket goes in "@[ ]"). */
    fun handle(name: String): String {
        val n = name.trim().removePrefix("~")
        return if (n.any { it.isWhitespace() }) "@[$n] " else "@$n "
    }

    /** The sender of a MeshCore channel message, which the radio writes as "Name: text"; null if there is none. */
    fun mcSender(text: String): String? {
        val i = text.indexOf(": ")
        if (i < 1 || i > 32) return null
        val s = text.substring(0, i)
        return if (s.contains('\n')) null else s
    }
}
