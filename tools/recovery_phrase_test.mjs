// Checks that the web client's recovery phrase (BIP39 -> seed -> master ->
// identity) and encrypted backup give the same result as
// maarse-crypto/src/recovery.rs (same test vectors).
//   node tools/recovery_phrase_test.mjs
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";

const html = readFileSync(new URL("../maarse-client/index.html", import.meta.url), "utf8");
const m = html.match(/\/\/ <recovery>([\s\S]*?)\/\/ <\/recovery>/);
if (!m) throw new Error("could not find the <recovery> block in index.html");
const enc = new TextEncoder(), dec = new TextDecoder();
const hex = b => Buffer.from(b).toString("hex");
const unhex = s => Uint8Array.from(s.match(/../g), x => parseInt(x, 16));
const R = new Function("enc", "dec", "hex", m[1] +
  "\nreturn { BIP39_WORDS, phraseFromEntropy, entropyFromPhrase, phraseSeed, masterSecret, identityFromPhrase, encryptBackup, decryptBackup };")(enc, dec, hex);

const eq = (a, b, what) => { if (a !== b) throw new Error(`${what}:\n  got  ${a}\n  want ${b}`); };

// The word list is the official one.
eq(createHash("sha256").update(R.BIP39_WORDS.join("\n") + "\n").digest("hex"),
   "2f5eed53a4727b4bf8880d8f3f199efc90e58503646d9ff8eff3a2ed3b24dbda", "SHA-256 of the word list");
console.log("OK  BIP39 word list (2048 words, official SHA-256)");

// Official BIP39 vectors (24 words, passphrase "TREZOR") — same as in Rust.
const TREZOR = [
  ["00".repeat(32), "abandon ".repeat(23) + "art",
   "bda85446c68413707090a52022edd26a1c9462295029f2e60cd7c4f2bbd3097170af7a4d73245cafa9c3cca8d561a7c3de6f5d4a10be8ed2a5e608d68f92fcc8"],
  ["7f".repeat(32), "legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth title",
   "bc09fca1804f7e69da93c2f2028eb238c227f2e9dda30cd63699232578480a4021b146ad717fbb7e451ce9eb835f43620bf5c514db0f8add49f5d121449d3e87"],
  ["80".repeat(32), "letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor acoustic bless",
   "c0c519bd0e91a2ed54357d9d1ebef6f5af218a153624cf4f2da911a0ed8f7a09e2ef61af0aca007096df430022f7a2b6fb91661a9589097069720d015e4e982f"],
  ["ff".repeat(32), "zoo ".repeat(23) + "vote",
   "dd48c104698c30cfe2b6142103248622fb7bb0ff692eebb00089b32d22484e1613912f0a5b694407be899ffd31ed3992c456cdf60f5d4564b8ba3f05a69890ad"],
  ["68a79eaca2324873eacc50cb9c6eca8cc68ea5d936f98787c60c7ebc74e6ce7c",
   "hamster diagram private dutch cause delay private meat slide toddler razor book happy fancy gospel tennis maple dilemma loan word shrug inflict delay length",
   "64c87cde7e12ecf6704ab95bb1408bef047c22db4cc7491c4271d170a1b213d20b385bc1588d9c7b38f1b39d415665b8a9030c9ec653d75e65f847d8fc1fc440"],
];
for (const [e, phrase, seed] of TREZOR) {
  eq(await R.phraseFromEntropy(unhex(e)), phrase, "entropy -> phrase");
  eq(hex(await R.entropyFromPhrase(phrase)), e, "phrase -> entropy");
  eq(hex(await R.phraseSeed(phrase, "TREZOR")), seed, "phrase -> seed");
}
console.log("OK  official BIP39 test vectors (TREZOR)");

// Our vector (maarse-crypto/src/recovery.rs, maarse_vector).
const id = await R.identityFromPhrase(("ABANDON  ".repeat(23) + "art\n"), "");
eq(hex(id.secret), "743d6ccda8a1826db3ef1df490b9d6d82f4b3920c76335447f21b261e4bf72525485ca54b88ea0c0358d84254ee7c92855aa5ec5fec21de960bf6f00996c80a4", "master secret");
eq(hex(id.pub), "04c10d960686af455a9cb72273370a85d566605017f1cc37d41abde3b1e5b53facc9b97b5a899e4332874acc655e5f792e806d6743b26049c90f868a3484e112", "public identity");
eq(id.addr, "6ef182b74279e05122977ced600ae3e6", "address");
console.log("OK  phrase -> identity in JavaScript = maarse-crypto (address 6ef182b7…)");

// Errors.
const fails = async (s, want) => {
  try { await R.entropyFromPhrase(s); } catch (e) { if (e.message.includes(want)) return; throw e; }
  throw new Error("was accepted: " + s);
};
await fails("abandon", "24 words");
await fails(TREZOR[1][1].replace("thank", "thnk"), "Word no. 3");
await fails(TREZOR[1][1].replace("title", "zoo"), "checksum");
console.log("OK  wrong phrases are rejected");

// Encrypted backup: round trip, wrong password and tampered header.
const data = { app: "maarse", v: 1, nicknames: { ab: "Zoë Æ" } };
const text = await R.encryptBackup(data, "correct horse battery", 100000);
eq(JSON.stringify(await R.decryptBackup(text, "correct horse battery")), JSON.stringify(data), "backup round trip");
for (const [bad, pass] of [[text, "wrong"], [text.replace('"iter":100000', '"iter":100001'), "correct horse battery"]]) {
  try { await R.decryptBackup(bad, pass); throw new Error("should fail"); } catch (e) { if (!e.message.includes("Wrong")) throw e; }
}
const full = JSON.parse(await R.encryptBackup(data, "x".repeat(12)));
eq(full.iter, 600000, "default rounds");
console.log("OK  encrypted backup (PBKDF2-SHA256 600,000 + AES-256-GCM)");
