package dk.maarse.app

import java.security.MessageDigest

/**
 * Safety numbers: the same computation as maarse-crypto::safety_number and
 * the web client. Each side's public identity (64 bytes: Ed25519 | X25519)
 * gives 30 digits (SHA-256 512 times); the number is both, the smaller one
 * first, so both people see the same 60 digits.
 */
object Safety {
    private const val CONTEXT = "maarse/safety-number/v0"
    private const val ITERATIONS = 512

    fun fingerprint(key: ByteArray): String {
        val md = MessageDigest.getInstance("SHA-256")
        md.update(CONTEXT.toByteArray())
        md.update(key)
        var h = md.digest()
        repeat(ITERATIONS - 1) {
            md.update(h)
            md.update(key)
            h = md.digest()
        }
        val s = StringBuilder()
        for (i in 0 until 6) {
            var v = 0L
            for (j in 0 until 5) v = v * 256 + (h[i * 5 + j].toInt() and 0xff)
            s.append((v % 100000).toString().padStart(5, '0'))
        }
        return s.toString()
    }

    /** 60 digits, the same whichever side computes it. */
    fun number(a: ByteArray, b: ByteArray): String {
        val fa = fingerprint(a)
        val fb = fingerprint(b)
        return if (fa <= fb) fa + fb else fb + fa
    }
}
