package dk.maarse.app

/**
 * The translation engine of [I18n], without anything from Android so it can be tested.
 * [texts] maps English text to its translation. An entry may hold `{0}`, `{1}` … or Java
 * format codes (`%d`, `%.1f`, `%s`) for the variable parts; those parts are translated too.
 */
class Translator(texts: Map<String, String>) {
    private class Pat(val re: Regex, val pre: String, val suf: String, val min: Int, val out: String)

    private val lock = Any()
    private val dict = HashMap<String, String>()
    private val pats: List<Pat>
    private val cache = HashMap<String, String>()

    init {
        val p = ArrayList<Pat>()
        for ((key, value) in texts) {
            if (value.isEmpty()) continue
            val k = norm(key)
            if (WILD.containsMatchIn(k)) p += compile(k, value) else dict[k] = value
        }
        p.sortByDescending { it.min }
        pats = p
    }

    /** Translates [text], keeping the white space around it. */
    fun translate(text: String): String {
        if (text.none { it in 'A'..'Z' || it in 'a'..'z' }) return text
        val lead = text.takeWhile { it.isWhitespace() }
        val tail = text.takeLastWhile { it.isWhitespace() }
        val body = text.trim()
        if (body.isEmpty()) return text
        val n = norm(body)
        val r = core(n)
        return if (r != n) lead + r + tail else text
    }

    private fun core(c: String): String {
        synchronized(lock) { cache[c]?.let { return it } }
        var r: String? = dict[c]
        if (r == null) {
            for (p in pats) {
                if (c.length < p.min || (p.pre.isNotEmpty() && !c.startsWith(p.pre)) || (p.suf.isNotEmpty() && !c.endsWith(p.suf))) continue
                val m = p.re.matchEntire(c) ?: continue
                r = INDEX.replace(p.out) { mm ->
                    val i = mm.groupValues[1].toInt()
                    if (i + 1 < m.groupValues.size) core(m.groupValues[i + 1]) else ""
                }
                break
            }
        }
        if (r == null && SENTENCE.containsMatchIn(c)) {
            // A paragraph made of several known sentences: translate each one.
            val parts = c.split(SENTENCE_SPLIT)
            if (parts.size > 1) {
                var any = false
                val out = parts.map { val y = core(it); if (y != it) any = true; y }
                if (any) r = out.joinToString(" ")
            }
        }
        val result = r ?: c
        synchronized(lock) {
            if (cache.size > 6000) cache.clear()
            cache[c] = result
        }
        return result
    }

    /** Turns "Sent {0} messages" or "%d KB" into a pattern that finds the variable parts. */
    private fun compile(key: String, value: String): Pat {
        val sb = StringBuilder("^")
        var last = 0
        var fixed = 0
        var pre = ""
        var first = true
        var positional = false
        for (m in WILD.findAll(key)) {
            val lit = key.substring(last, m.range.first).replace("%%", "%")
            if (first) { pre = lit; first = false }
            sb.append(Regex.escape(lit)).append("(.+?)")
            fixed += lit.length
            last = m.range.last + 1
            if (m.value.startsWith("{")) positional = true
        }
        val suf = key.substring(last).replace("%%", "%")
        sb.append(Regex.escape(suf)).append("$")
        fixed += suf.length
        // The translation refers to the variable parts by {n}, or by format codes in order.
        var out = value
        if (!positional) {
            var n = 0
            out = FORMAT.replace(value) { "{" + (n++) + "}" }.replace("%%", "%")
        }
        return Pat(Regex(sb.toString()), pre, suf, fixed, out)
    }

    companion object {
        private val WILD = Regex("""\{\d+\}|%(?:\.\d+)?[dsf]|%\d*d""")
        private val FORMAT = Regex("""%(?:\.\d+)?[dsf]|%\d*d""")
        private val INDEX = Regex("""\{(\d+)\}""")
        private val SPACES = Regex("\\s+")
        private val SENTENCE = Regex("""[.!?] \S""")
        private val SENTENCE_SPLIT = Regex("""(?<=[.!?]) (?=\S)""")

        private fun norm(s: String) = s.replace(SPACES, " ").trim()

        /** Replaces `{0}`, `{1}` … in [text] by [args]. */
        fun fill(text: String, args: Array<out Any?>): String =
            INDEX.replace(text) { m -> args.getOrNull(m.groupValues[1].toInt())?.toString() ?: "" }
    }
}
