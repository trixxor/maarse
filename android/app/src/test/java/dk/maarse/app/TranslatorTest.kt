package dk.maarse.app

import org.junit.Assert.assertEquals
import org.junit.Test

class TranslatorTest {
    private val t = Translator(
        mapOf(
            "Settings" to "Indstillinger",
            "Sent {0} messages" to "Sendte {0} beskeder",
            "Status: {0}" to "Status: {0}",
            "Connected" to "Forbundet",
            "%.1f KB/s · %d packets" to "%.1f KB/s · %d pakker",
            "Waiting. Sent." to "Venter. Sendt.",
            "Waiting." to "Venter.",
            "Sent." to "Sendt.",
            "{0} hop{1}" to "{0} hop",
        ),
    )

    @Test fun exactText() = assertEquals("Indstillinger", t.translate("Settings"))

    @Test fun keepsSpacesAround() = assertEquals("  Indstillinger\n", t.translate("  Settings\n"))

    @Test fun unknownStaysEnglish() = assertEquals("Something else", t.translate("Something else"))

    @Test fun numbersAreNotTouched() = assertEquals("12 3", t.translate("12 3"))

    @Test fun placeholders() = assertEquals("Sendte 5 beskeder", t.translate("Sent 5 messages"))

    @Test fun variablePartIsTranslatedToo() = assertEquals("Status: Forbundet", t.translate("Status: Connected"))

    @Test fun formatCodes() = assertEquals("1.5 KB/s · 3 pakker", t.translate("1.5 KB/s · 3 packets"))

    @Test fun sentencesOfAParagraph() = assertEquals("Venter. Sendt.", t.translate("Waiting. Sent."))

    @Test fun partlyKnownParagraph() = assertEquals("Venter. Unknown thing.", t.translate("Waiting. Unknown thing."))

    @Test fun dropsUnusedPart() = assertEquals("3 hop", t.translate("3 hops"))

    @Test fun fillsArguments() = assertEquals("a b", Translator.fill("{0} {1}", arrayOf("a", "b")))
}
