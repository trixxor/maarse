// Tjekker, at webklientens JavaScript-udgave af sikkerhedsnummeret giver
// samme tal som maarse-crypto (testvektoren i maarse-crypto/src/lib.rs).
//   node tools/safety_number_test.mjs
import { readFileSync } from "node:fs";
import { createHash, randomBytes } from "node:crypto";

const html = readFileSync(new URL("../maarse-client/index.html", import.meta.url), "utf8");
const m = html.match(/\/\/ <sikkerhedsnummer>([\s\S]*?)\/\/ <\/sikkerhedsnummer>/);
if (!m) throw new Error("fandt ikke <sikkerhedsnummer>-blokken i index.html");
const { sha256, safetyNumber } = new Function(m[1] + "\nreturn { sha256, safetyNumber };")();

const unhex = s => Uint8Array.from(s.match(/../g), x => parseInt(x, 16));
const hex = b => Buffer.from(b).toString("hex");

// SHA-256 mod Node's egen.
for (const n of [0, 1, 55, 56, 63, 64, 65, 96, 200]) {
  const d = randomBytes(n);
  if (hex(sha256(d)) !== createHash("sha256").update(d).digest("hex")) throw new Error("sha256 fejler ved længde " + n);
}

const A = unhex("8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c" +
                "a4e09292b651c278b9772c569f5fa9bb13d906b46ab68c9df9dc2b4409f8a209");
const B = unhex("8139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b394" +
                "ce8d3ad1ccb633ec7b70c17814a5c76ecd029685050d344745ba05870e587d59");
const want = "490814315733469534137526241006891633028785095264240772532833";
const got = safetyNumber(A, B);
if (got !== want) throw new Error(`forkert sikkerhedsnummer:\n  ${got}\n  ${want}`);
if (safetyNumber(B, A) !== want) throw new Error("afhænger af rækkefølgen");
console.log("OK  sikkerhedsnummer i JavaScript = maarse-crypto");
