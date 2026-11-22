package dk.maarse.app

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import org.json.JSONObject
import java.io.File
import java.util.Locale

/**
 * Language support. The English text in the code is the lookup key, so no text is renamed:
 * a language file (`i18n/<code>.json`, shared with the web client) maps English text to the
 * translation. A text that is missing in the language stays English.
 *
 * Entries may hold `{0}`, `{1}` … or Java format codes (`%d`, `%.1f`, `%s`) for the variable
 * parts; the variable parts are translated too. The language follows the device unless the
 * user chose one in Settings.
 *
 * Language files are built into the app (assets/i18n). A file placed in the app's
 * `files/i18n/` folder replaces a built-in one or adds a language.
 */
object I18n {
    const val AUTO = "auto"

    class Lang(val code: String, val name: String)

    private var app: Context? = null

    @Volatile
    private var translator: Translator? = null

    /** Every available language (English is always there and is not listed). */
    var languages: List<Lang> = emptyList()
        private set

    /** What the user chose: [AUTO] or a language code. */
    var choice by mutableStateOf(AUTO)
        private set

    /** The language in use. Reading it in a composable makes the screen redraw when it changes. */
    var active by mutableStateOf("en")
        private set

    fun init(context: Context) {
        app = context.applicationContext
        languages = scan(context)
        choice = prefs(context).getString("lang", AUTO) ?: AUTO
        apply(context)
    }

    fun setChoice(context: Context, code: String) {
        prefs(context).edit().putString("lang", code).apply()
        choice = code
        apply(context)
    }

    private fun prefs(context: Context) = context.getSharedPreferences("maarse-ui", Context.MODE_PRIVATE)

    private fun userFile(context: Context, code: String) = File(File(context.filesDir, "i18n"), "$code.json")

    private fun readFile(context: Context, code: String): String? {
        val user = userFile(context, code)
        if (user.isFile) runCatching { return user.readText().also { JSONObject(it) } }
        return runCatching { context.assets.open("i18n/$code.json").bufferedReader().use { it.readText() } }.getOrNull()
    }

    private fun scan(context: Context): List<Lang> {
        val codes = sortedSetOf<String>()
        runCatching { context.assets.list("i18n")?.forEach { if (it.endsWith(".json")) codes += it.removeSuffix(".json") } }
        File(context.filesDir, "i18n").listFiles()?.forEach { if (it.name.endsWith(".json")) codes += it.name.removeSuffix(".json") }
        return codes.mapNotNull { code ->
            val text = readFile(context, code) ?: return@mapNotNull null
            val name = runCatching { JSONObject(text).getJSONObject("meta").getString("name") }.getOrDefault(code)
            Lang(code, name)
        }
    }

    /** The language code to use: the choice, or the device language if we have it, else English. */
    private fun resolve(): String {
        if (choice != AUTO) return choice
        val have = languages.map { it.code }.toSet()
        val wanted = android.content.res.Resources.getSystem().configuration.locales
        for (i in 0 until wanted.size()) {
            val l = wanted[i]
            val tag = l.toLanguageTag().lowercase(Locale.ROOT)
            if (l.language == "en") return "en"
            if (tag in have) return tag
            if (l.language in have) return l.language
        }
        return "en"
    }

    private fun apply(context: Context) {
        val code = resolve()
        var texts: JSONObject? = null
        if (code != "en") {
            texts = readFile(context, code)?.let { runCatching { JSONObject(it).getJSONObject("texts") }.getOrNull() }
        }
        translator = texts?.let { t -> Translator(t.keys().asSequence().associateWith { k -> t.optString(k) }) }
        active = if (texts == null) "en" else code
    }

    /** Translates [text]; `{0}`, `{1}` in it are replaced by [args]. */
    fun tr(text: String, vararg args: Any?): String {
        @Suppress("UNUSED_VARIABLE") val readForCompose = active
        var s = text
        val t = translator
        if (t != null) s = t.translate(s)
        if (args.isNotEmpty()) s = Translator.fill(s, args)
        return s
    }
}

/** Translates a text; see [I18n]. */
fun tr(text: String, vararg args: Any?): String = I18n.tr(text, *args)
